use crate::config::IptvConfig;


use anyhow::{anyhow, Result};
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
use std::{
    time::{Duration, SystemTime, UNIX_EPOCH},
};
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

pub(crate) async fn get_channels(
    args: &IptvConfig,
    need_epg: bool,
) -> Result<Vec<Channel>> {
    info!("Obtaining channels");

    let start_time = std::time::Instant::now();

    let user = args.user.as_str();
    let passwd = args.passwd.as_str();
    let mac = args.mac.as_str();
    let imei = args.imei.as_deref().unwrap_or("default_imei");
    let ip = args.ip.as_deref().unwrap_or("0.0.0.0");

    let client = get_client_with_if(args.interface.as_deref())?;

    let base_url = get_base_url(&client, args).await?;

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
    let url =
        reqwest::Url::parse_with_params(format!("{base_url}/EPG/oauth/v2/token").as_str(), params)?;
    let _response = client.get(url).send().await?.error_for_status()?;

    let url = reqwest::Url::parse(format!("{base_url}/EPG/jsp/getchannellistHWCTC.jsp").as_str())?;

    let response = client.get(url).send().await?.error_for_status()?;

    let res = response.text().await?;
            
    // 使用更精确的正则表达式匹配频道信息
    let channel_pattern = Regex::new(r#"(?m)Authentication.CTCSetConfig([^"]*)ChannelID="([^"]*)",ChannelName="([^"]*)",UserChannelID="([^"]*)",ChannelURL="([^|]*)\|([^"]*)",(.*?)TimeShiftURL="([^"]*)""#)?;

    // iRet = Authentication.CTCSetConfig('Channel','ChannelID="10799",ChannelName="深圳都市",UserChannelID="1002",ChannelURL="igmp://239.77.1.176:5146|rtsp://183.59.160.198/PLTV/88888895/224/3221228036/10000100000000060000000007005128_0.smil?rrsip=",TimeShift="1",TimeShiftLength="7200",ChannelSDP="igmp://239.77.1.176:5146|rtsp://183.59.160.198/PLTV/88888895/224/3221228036/10000100000000060000000007005128_0.smil",TimeShiftURL="rtsp://183.59.160.198/PLTV/88888895/224/3221228036/10000100000000060000000007005128_0.smil?rrsip=125.88.70.140,rrsip=125.88.104.40&zoneoffset=0&icpid=&limitflux=-1&limitdur=-1&tenantId=8601&GuardEncType=2&accountinfo=%7E%7EV2.0%7EqaHhGruMstwIkaFtk0MP7A%7EGLOFB5O3kvJE5I3TTb2pEnLv4APVPB7NMPEjL0UknV8dFj2VERn_IP31ISGmAOZRDmdJGvBiqO7hRNodRy4KDtUxzPGT5g3dzUoMzbpT76I%7EExtInfoPC2ZKLw95m5z2wHEEFeSaQ%3A20251201003536%2C8982293%2C10.101.36.213%2C20251201003536%2C31000100000000050000000000440672%2C8982293%2C-1%2C0%2C1%2C%2C%2C7%2C%2C%2C%2C4%2C%2C10000100000000060000000007005128_0%2CEND",ChannelType="1",IsHDChannel="1",PreviewEnable="0",ChannelPurchased="1",ChannelLocked="0",ChannelLogURL="",PositionX="",PositionY="",BeginTime="0",Interval="",Lasting="",ActionType="1",FCCEnable="0",ChannelFECPort="5145"');

    let mut channels = Vec::new();

    // 首先尝试使用新的精确正则表达式
    for cap in channel_pattern.captures_iter(&res) {
        let channel_id = cap[2].to_string();
        let channel_name = cap[3].to_string().replace('＋', "+").replace(' ', "");
        let user_channel_id = cap[4].to_string();
        let igmp = cap[5].to_string();
        // let rtsp = cap[6].to_string();
        let time_shift_url = cap[8].to_string();

        // 将 UserChannelID 转换为 u64
        let channel_id = channel_id.parse::<u64>()
            .unwrap_or_else(|_| {
                // 如果解析失败，使用哈希值作为备用方案
                channel_id.as_str().chars().map(|c| c as u64).sum()
            });


        debug!("igmp {} ", igmp);

        let channel = Channel {
            id: channel_id,
            user_channel_id: user_channel_id,
            name: channel_name,
            rtsp: time_shift_url, // 或者根据实际情况决定使用哪个 URL
            igmp: igmp, // 根据你的数据源设置 IGMP 地址
            epg: Vec::new(), // 初始化为空，后续可以填充 EPG 数据
        };
        channels.push(channel);
    }
        
        
    info!("Got {} channel(s)", channels.len());


    if !need_epg {
        let elapsed = start_time.elapsed();
        println!("📡 获取频道列表... in {:?}", elapsed);
        return Ok(channels);
    }

    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();

    let mut tasks = JoinSet::new();

    for channel in channels.into_iter() {
        let params = [
            ("channelId", format!("{}", channel.id)),
            ("begin", format!("{}", now - 86400000 * 7)),
            ("end", format!("{}", now + 86400000 * 2)),
        ];
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

    let elapsed = start_time.elapsed();
    println!("📋 获取epg信息... in {:?}", elapsed);

    Ok(channels)
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
