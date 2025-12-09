use crate::config::IptvConfig;

use chrono::{ NaiveDate,  TimeZone, Utc};
use chrono::Duration as ChronoDuration;
use anyhow::{Result, Context, anyhow};
use des::{
    cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyInit},
    TdesEde3,
};
#[cfg(not(any(target_os = "android", target_os = "fuchsia", target_os = "linux")))]
use local_ip_address::list_afinet_netifas;
use log::{debug, info};
use rand::Rng;
use regex_lite::Regex;
use reqwest::Client;
use serde::Deserialize;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::task::JoinSet;

fn get_client_with_if(#[allow(unused_variables)] if_name: Option<&str>) -> Result<Client> {
    let timeout = Duration::new(5, 0);
    #[allow(unused_mut)]
    let mut client = Client::builder().timeout(timeout).cookie_store(true);

    #[cfg(not(any(target_os = "android", target_os = "fuchsia", target_os = "linux")))]
    if let Some(i) = if_name {
        let network_interfaces = list_afinet_netifas()?;
        for (name, ip) in network_interfaces.iter() {
            debug!("{}: {}", name, ip);
            if name == i {
                client = client.local_address(ip.to_owned());
                break;
            }
        }
    }

    #[cfg(any(target_os = "android", target_os = "fuchsia", target_os = "linux"))]
    if let Some(i) = if_name {
        client = client.interface(i);
    }

    Ok(client.build()?)
}

async fn get_base_url(client: &Client, args: &IptvConfig) -> Result<String> {
    let user = args.user.as_str();

    let params = [("Action", "Login"), ("return_type", "1"), ("UserID", user)];

    let url = reqwest::Url::parse_with_params(
        "http://eds.iptv.gd.cn:8082/EDS/jsp/AuthenticationURL",
        params,
    )?;

    let response = client.get(url).send().await?.error_for_status()?;

    let epgurl = reqwest::Url::parse(response.json::<AuthJson>().await?.epgurl.as_str())?;
    let base_url = format!(
        "{}://{}:{}",
        epgurl.scheme(),
        epgurl.host_str().ok_or(anyhow!("no host"))?,
        epgurl.port_or_known_default().ok_or(anyhow!("no port"))?,
    );
    debug!("Got base_url {base_url}");
    Ok(base_url)
}

pub(crate) struct Program {
    pub(crate) start: i64,
    pub(crate) stop: i64,
    pub(crate) title: String,
    pub(crate) desc: String,
}

pub(crate) struct Channel {
    pub(crate) id: u64,
    pub(crate) user_channel_id: String,
    pub(crate) name: String,
    pub(crate) rtsp: String,
    pub(crate) igmp: String,
    pub(crate) epg: Vec<Program>,
}

#[derive(Deserialize)]
struct AuthJson {
    epgurl: String,
}

#[derive(Deserialize)]
struct TokenJson {
    #[serde(rename = "EncryToken")]
    encry_token: String,
}

#[derive(Deserialize)]
struct PlaybillList {
    #[serde(rename = "playbillLites")]
    list: Vec<Bill>,
}

#[derive(Deserialize)]
struct Bill {
    name: String,
    #[serde(rename = "startTime")]
    start_time: i64,
    #[serde(rename = "endTime")]
    end_time: i64,
}
/// 登录 IPTV 系统，返回认证后的 client 和 base_url
pub(crate) async fn login_iptv(args: &IptvConfig) -> Result<(reqwest::Client, String)> {
    static mut CACHED_RESULT: Option<(reqwest::Client, String, Instant)> = None;
    const CACHE_DURATION: Duration = Duration::from_secs(1800);
    
    unsafe {
        // 获取原始指针
        let cached_ptr = &raw const CACHED_RESULT;
        
        // 解引用指针获取 Option
        match *cached_ptr {
            Some((ref client, ref base_url, cached_time)) => {
                if cached_time.elapsed() < CACHE_DURATION {
                    info!("使用缓存的登录会话");
                    return Ok((client.clone(), base_url.clone()));
                }
            }
            None => {}
        }
    }
    
    info!("开始登录 IPTV 系统");

    let start_time = std::time::Instant::now();
    
    let user = args.user.as_str();
    let passwd = args.passwd.as_str();
    let mac = args.mac.as_str();
    let imei = args.imei.as_deref().unwrap_or("default_imei");
    let ip = args.ip.as_deref().unwrap_or("0.0.0.0");

    // 创建客户端
    let client = get_client_with_if(args.interface.as_deref())?;

    // 获取基础 URL
    let base_url = get_base_url(&client, args).await?;

    // 第一步：获取 token
    let params = [
        ("response_type", "EncryToken"),
        ("client_id", "smcphone"),
        ("userid", user),
    ];
    let url = reqwest::Url::parse_with_params(
        format!("{base_url}/EPG/oauth/v2/authorize").as_str(),
        params,
    )?;
    let response = client.get(url).send().await?.error_for_status()?;
    let token = response.json::<TokenJson>().await?.encry_token;
    debug!("Got token {token}");

    // 第二步：生成认证信息
    let enc = ecb::Encryptor::<TdesEde3>::new_from_slice(
        format!("{:X}", md5::compute(passwd.as_bytes()))[0..24].as_bytes(),
    );
    let enc = match enc {
        Ok(enc) => Ok(enc),
        Err(e) => Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!("Encrypt error {e}"),
        )),
    }?;
    
    let data = format!(
        "{}${token}${user}${imei}${ip}${mac}$$CTC",
        rand::thread_rng().gen_range(0..10000000),
    );
    let auth = hex::encode_upper(enc.encrypt_padded_vec_mut::<Pkcs7>(data.as_bytes()));
    debug!("Got auth {auth}");

    // 第三步：获取访问令牌
    let params = [
        ("client_id", "smcphone"),
        ("DeviceType", "deviceType"),
        ("UserID", user),
        ("DeviceVersion", "deviceVersion"),
        ("userdomain", "2"),
        ("datadomain", "3"),
        ("accountType", "1"),
        ("authinfo", auth.as_str()),
        ("grant_type", "EncryToken"),
    ];
    let url = reqwest::Url::parse_with_params(
        format!("{base_url}/EPG/oauth/v2/token").as_str(),
        params,
    )?;
    
    let _response = client.get(url).send().await?.error_for_status()?;
    
    // 缓存结果
    unsafe {
        CACHED_RESULT = Some((client.clone(), base_url.clone(), Instant::now()));
    }
    
    let elapsed = start_time.elapsed();
    info!("成功登录 IPTV 系统，耗时: {:?}", elapsed);
    
    Ok((client, base_url))
}

/// 获取频道列表（不包含 EPG）
pub(crate) async fn get_channel_list(
    client: &reqwest::Client,
    base_url: &str,
) -> Result<Vec<Channel>> {
    info!("Fetching channel list");
    let start_time = std::time::Instant::now();
    
    let url = reqwest::Url::parse(
        format!("{}/EPG/jsp/getchannellistHWCTC.jsp", base_url).as_str()
    )?;
    
    let response = client.get(url).send().await?.error_for_status()?;
    let res = response.text().await?;
    
    let channel_pattern = Regex::new(
        r#"(?m)Authentication.CTCSetConfig([^"]*)ChannelID="([^"]*)",ChannelName="([^"]*)",UserChannelID="([^"]*)",ChannelURL="([^|]*)\|([^"]*)",(.*?)TimeShiftURL="([^"]*)""#
    )?;
    
    let mut channels = Vec::new();
    
    for cap in channel_pattern.captures_iter(&res) {
        let channel_id = cap[2].to_string();
        let channel_name = cap[3].to_string().replace('＋', "+").replace(' ', "").replace('-', "");
        let user_channel_id = cap[4].to_string();
        let igmp = cap[5].to_string();
        let time_shift_url = cap[8].to_string();
        
        let channel_id = channel_id.parse::<u64>()
            .unwrap_or_else(|_| {
                channel_id.as_str().chars().map(|c| c as u64).sum()
            });
        
        debug!("Found channel: {} (ID: {})", channel_name, channel_id);
        
        let channel = Channel {
            id: channel_id,
            user_channel_id,
            name: channel_name,
            rtsp: time_shift_url,
            igmp,
            epg: Vec::new(),
        };
        channels.push(channel);
    }
    
    info!("Got {} channel(s)", channels.len());
    let elapsed = start_time.elapsed();
    println!("📡 获取频道列表... in {:?}", elapsed);
    Ok(channels)
}

pub(crate) async fn get_channels(
    args: &IptvConfig,

) -> Result<Vec<Channel>> {
    info!("Obtaining channels");

    // 1. 登录获取认证后的客户端
    let (client, base_url) = login_iptv(args).await?;
    
    // 2. 获取频道列表
    let channels = get_channel_list(&client, &base_url).await?;

    Ok(channels)
}

pub(crate) async fn get_channels_epg(
    args: &IptvConfig,

) -> Result<Vec<Channel>> {

    let start_time = std::time::Instant::now();

    // 1. 登录获取认证后的客户端
    let (client, base_url) = login_iptv(args).await?;
    
    // 2. 获取频道列表
    let channels = get_channel_list(&client, &base_url).await?;

    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let begin_timestamp = now - 86400000 * 7;
    let end_timestamp = now + 86400000 * 2;
    let mut tasks = JoinSet::new();

    for channel in channels.into_iter() {
        let params = [
            ("channelId", format!("{}", channel.id)),
            ("begin", format!("{}", begin_timestamp)),
            ("end", format!("{}", end_timestamp)),
        ];

        info!("请求参数:channle_id={} begin={}, end={}",channel.id  , begin_timestamp, end_timestamp);

        let url = reqwest::Url::parse_with_params(
            format!("{base_url}/EPG/jsp/iptvsnmv3/en/play/ajax/_ajax_getPlaybillList.jsp").as_str(),
            params,
        )?;
        let client = client.clone();
        tasks.spawn(async move { (client.get(url).send().await, channel) });
    }
    let mut channels = vec![];
    while let Some(Ok((Ok(res), mut channel))) = tasks.join_next().await {
        if let Ok(play_bill_list) = res.json::<PlaybillList>().await {
            debug!("获得节目单信息:channle_id={} begin={}, end={}, 数量={}",channel.id  , begin_timestamp, end_timestamp, play_bill_list.list.len());
            for bill in play_bill_list.list.into_iter() {
                channel.epg.push(Program {
                    start: bill.start_time,
                    stop: bill.end_time,
                    title: bill.name.clone(),
                    desc: bill.name,
                })
            }
        }
        channels.push(channel);
    }

    let elapsed: Duration = start_time.elapsed();
    println!("📋 获取epg信息... in {:?}", elapsed);

    Ok(channels)
}

/// 获取指定频道在指定日期的 EPG 数据
pub(crate) async fn get_channel_date_epg(
    args: &IptvConfig,
    channel_id: u64,
    date: &str,
) -> Result<Channel> {
    info!("获取频道 {} 在 {} 的 EPG 数据", channel_id, date);

    // 1. 登录获取认证后的客户端
    let (client, base_url) = login_iptv(args).await?;


    let mut target_channel  = Channel {
        id: channel_id,
        user_channel_id: String::new(),
        name: String::new(),
        rtsp: String::new(),
        igmp: String::new(),
        epg: Vec::new(),
    };

    let start_time = std::time::Instant::now();

    // 3. 解析日期，获取时间范围
    let (begin_timestamp, end_timestamp) = cal_date_range(&date)?;

    debug!("请求参数: channel_id={} date={} begin={} end={}", 
          channel_id, date, begin_timestamp, end_timestamp);

    // 4. 只获取目标频道的 EPG
    let params = [
        ("channelId", format!("{}", channel_id)),
        ("begin", format!("{}", begin_timestamp)),
        ("end", format!("{}", end_timestamp)),
    ];

    let url = reqwest::Url::parse_with_params(
        format!("{}/EPG/jsp/iptvsnmv3/en/play/ajax/_ajax_getPlaybillList.jsp", base_url).as_str(),
        params,
    )?;

    // 5. 发送请求
    let response = client.get(url).send().await?.error_for_status()?;
    
    let play_bill_list: PlaybillList = response.json().await?;

    // 8. 填充 EPG 数据
    target_channel.epg.clear(); // 清空现有 EPG 数据
    for bill in play_bill_list.list.into_iter() {
        debug!("EPG: {} - {}", bill.start_time, bill.name);
        target_channel.epg.push(Program {
            start: bill.start_time,
            stop: bill.end_time,
            title: bill.name.clone(),
            desc: bill.name,
        });
    }

    let elapsed = start_time.elapsed();
    info!("📋 获取频道 {} 在 {} 的 EPG 数据完成，耗时 {:?}，共 {} 条节目", 
          channel_id, date, elapsed, target_channel.epg.len());

    Ok(target_channel)
}


/// 解析日期字符串，总是返回当天0点时间
fn cal_date_range(date_str: &str) -> Result<(i64, i64)> {
    // 尝试多种日期格式
    let formats = [
        "%Y%m%d",        // 20241201
        "%Y-%m-%d",      // 2024-12-01
        "%Y/%m/%d",      // 2024/12/01
        "%d-%m-%Y",      // 01-12-2024
        "%d/%m/%Y",      // 01/12/2024
    ];
    
    let mut parsed_date = None;
    for &format in &formats {
        if let Ok(date) = NaiveDate::parse_from_str(date_str, format) {
            parsed_date = Some(date);
            break;
        }
    }
    
    let naive_date = parsed_date.ok_or_else(|| 
        anyhow!("无法解析日期字符串: {}。支持的格式: YYYYMMDD, YYYY-MM-DD, YYYY/MM/DD, DD-MM-YYYY, DD/MM/YYYY", date_str)
    )?;
    
    // 当天0点的UTC时间
    let start_datetime = Utc.from_utc_datetime(&naive_date.and_hms_opt(0, 0, 0)
        .context("无法创建当天0点时间")?);
    
    // 次日0点
    let end_datetime = start_datetime + ChronoDuration::days(1);
    
    // 转换为毫秒时间戳
    Ok((start_datetime.timestamp_millis(), end_datetime.timestamp_millis()))
    

}


pub(crate) async fn get_icon(args: &IptvConfig, id: &str) -> Result<Vec<u8>> {
    let client = get_client_with_if(args.interface.as_deref())?;

    let base_url = get_base_url(&client, args).await?;

    let url = reqwest::Url::parse(&format!(
        "{base_url}/EPG/jsp/iptvsnmv3/en/list/images/channelIcon/{}.png",
        id
    ))?;

    let response = client.get(url).send().await?.error_for_status()?;
    Ok(response.bytes().await?.to_vec())
}
