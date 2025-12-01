use actix_web::{
    get,
    web::{Data, Path},
    App, HttpRequest, HttpResponse, HttpServer, Responder,
};
use anyhow::{anyhow, Result};

use std::path::PathBuf;
use chrono::{FixedOffset, TimeZone, Utc};
use log::{debug, info};
use reqwest::Client;
use std::{

    io::{BufWriter, Cursor, Read},


    sync::Mutex,
};
use xml::{
    reader::XmlEvent as XmlReadEvent,
    writer::{EmitterConfig, XmlEvent as XmlWriteEvent},
    EventReader,
};

mod args;
use args::Args;

mod config;
use config::YamlConfig;


mod iptv;
use iptv::{get_channels, get_icon, Channel};


static OLD_PLAYLIST: Mutex<Option<String>> = Mutex::new(None);
static OLD_XMLTV: Mutex<Option<String>> = Mutex::new(None);

/// 合并后的应用配置
#[derive(Clone)]
pub struct AppConfig {
    pub cli_args: Args,
    pub yaml_config: YamlConfig,
}

impl AppConfig {
    pub fn new(cli_args: Args) -> Result<Self, Box<dyn std::error::Error>> {
        // 验证命令行参数
        cli_args.validate()?;

        // 加载 YAML 配置
        let config_path = PathBuf::from(&cli_args.config_file);
        let yaml_config = YamlConfig::from_file(&config_path)?;

        Ok(AppConfig {
            cli_args,
            yaml_config,
        })
    }
}


fn to_xmltv_time(unix_time: i64) -> Result<String> {
    match Utc.timestamp_millis_opt(unix_time) {
        chrono::LocalResult::Single(t) => Ok(t
            .with_timezone(&FixedOffset::east_opt(8 * 60 * 60).ok_or(anyhow!(""))?)
            .format("%Y%m%d%H%M%S")
            .to_string()),
        _ => Err(anyhow!("fail to parse time")),
    }
}

fn to_xmltv<R: Read>(channels: Vec<Channel>, extra: Option<EventReader<R>>) -> Result<String> {
    let mut buf = BufWriter::new(Vec::new());
    let mut writer = EmitterConfig::new()
        .perform_indent(false)
        .create_writer(&mut buf);
    writer.write(
        XmlWriteEvent::start_element("tv")
            .attr("generator-info-name", "iptv-proxy")
            .attr("source-info-name", "iptv-proxy"),
    )?;
    for channel in channels.iter() {
        writer.write(
            XmlWriteEvent::start_element("channel").attr("id", &format!("{}", channel.id)),
        )?;
        writer.write(XmlWriteEvent::start_element("display-name"))?;
        writer.write(XmlWriteEvent::characters(&channel.name))?;
        writer.write(XmlWriteEvent::end_element())?;
        writer.write(XmlWriteEvent::end_element())?;
    }
    if let Some(extra) = extra {
        for e in extra {
            match e {
                Ok(XmlReadEvent::StartElement {
                    name, attributes, ..
                }) => {
                    let name = name.to_string();
                    let name = name.as_str();
                    if name != "channel"
                        && name != "display-name"
                        && name != "desc"
                        && name != "title"
                        && name != "sub-title"
                        && name != "programme"
                    {
                        continue;
                    }
                    let name = if name == "title" {
                        let mut iter = attributes.iter();
                        loop {
                            let attr = iter.next();
                            if attr.is_none() {
                                break "title";
                            }
                            let attr = attr.unwrap();
                            if attr.name.to_string() == "lang" && attr.value != "chi" {
                                break "title_extra";
                            }
                        }
                    } else {
                        name
                    };
                    let mut tag = XmlWriteEvent::start_element(name);
                    for attr in attributes.iter() {
                        tag = tag.attr(attr.name.borrow(), &attr.value);
                    }
                    writer.write(tag)?;
                }
                Ok(XmlReadEvent::Characters(content)) => {
                    writer.write(XmlWriteEvent::characters(&content))?;
                }
                Ok(XmlReadEvent::EndElement { name }) => {
                    let name = name.to_string();
                    let name = name.as_str();
                    if name != "channel"
                        && name != "display-name"
                        && name != "desc"
                        && name != "title"
                        && name != "sub-title"
                        && name != "programme"
                    {
                        continue;
                    }
                    writer.write(XmlWriteEvent::end_element())?;
                }
                _ => {}
            }
        }
    }
    for channel in channels.iter() {
        for epg in channel.epg.iter() {
            writer.write(
                XmlWriteEvent::start_element("programme")
                    .attr("start", &format!("{} +0800", to_xmltv_time(epg.start)?))
                    .attr("stop", &format!("{} +0800", to_xmltv_time(epg.stop)?))
                    .attr("channel", &format!("{}", channel.id)),
            )?;
            writer.write(XmlWriteEvent::start_element("title").attr("lang", "chi"))?;
            writer.write(XmlWriteEvent::characters(&epg.title))?;
            writer.write(XmlWriteEvent::end_element())?;
            if !epg.desc.is_empty() {
                writer.write(XmlWriteEvent::start_element("desc"))?;
                writer.write(XmlWriteEvent::characters(&epg.desc))?;
                writer.write(XmlWriteEvent::end_element())?;
            }
            writer.write(XmlWriteEvent::end_element())?;
        }
    }
    writer.write(XmlWriteEvent::end_element())?;
    Ok(String::from_utf8(buf.into_inner()?)?)
}

async fn parse_extra_xml(url: &str) -> Result<EventReader<Cursor<String>>> {
    let client = Client::builder().build()?;
    let url = reqwest::Url::parse(url)?;
    let response = client.get(url).send().await?.error_for_status()?;
    let xml = response.text().await?;
    let reader = Cursor::new(xml);
    Ok(EventReader::new(reader))
}

#[get("/xmltv")]
async fn xmltv(config: Data<YamlConfig>, _req: HttpRequest) -> impl Responder {
    debug!("Get EPG");
    // let scheme = req.connection_info().scheme().to_owned();
    // let host = req.connection_info().host().to_owned();
    let extra_xml = match &config.m3u8.extra_xmltv {
        Some(u) => parse_extra_xml(u).await.ok(),
        None => None,
    };
    let xml = get_channels(&config.iptv, true)
        .await
        .and_then(|ch| to_xmltv(ch, extra_xml));
    match xml {
        Err(e) => {
            if let Some(old_xmltv) = OLD_XMLTV.try_lock().ok().and_then(|f| f.to_owned()) {
                HttpResponse::Ok().content_type("text/xml").body(old_xmltv)
            } else {
                HttpResponse::InternalServerError().body(format!("Error getting channels: {}", e))
            }
        }
        Ok(xml) => HttpResponse::Ok().content_type("text/xml").body(xml),
    }
}


async fn parse_extra_playlist(url: &str) -> Result<String> {
    let client = Client::builder().build()?;
    info!("开始解析额外播放列表: {}", url);

    let url = reqwest::Url::parse(url)?;
    let response = client.get(url).send().await?.error_for_status()?;
    Ok(response
        .text()
        .await?
        .strip_prefix("#EXTM3U")
        .map_or(String::from(""), |s| s.to_owned()))
}

#[get("/logo/{id}.png")]
async fn logo(config: Data<YamlConfig>, path: Path<String>) -> impl Responder {
    debug!("Get logo");
    match get_icon(&config.iptv, &path).await {
        Ok(icon) => HttpResponse::Ok().content_type("image/png").body(icon),
        Err(e) => HttpResponse::NotFound().body(format!("Error getting channels: {}", e)),
    }
}

/// 格式化频道名称（带详细日志，包含清理功能）
pub fn format_channel_name(name: &str, config: &YamlConfig) -> String {
    debug!("开始处理频道名称: {}", name);
    
    // 1. 先应用清理规则
    let mut cleaned = name.to_string();
    // 如果有自定义顺序需求，可以在这里排序
    let mut patterns = config.name_clean.clone();
    
    // 按长度从长到短排序（优先处理复合词）
    patterns.sort_by(|a, b| b.len().cmp(&a.len()));
    patterns.dedup(); // 去重
    
    debug!("清理模式（按长度排序）: {:?}", patterns);
    
    for pattern in &patterns {
        let original = cleaned.clone();
        cleaned = cleaned.replace(pattern, "");
        if cleaned != original {
            info!("移除 '{}': {} -> {}", pattern, original, cleaned);
        }
    }
    
    cleaned = cleaned.trim().to_string();
    
    // 2. 应用名称映射（如果有）
    if let Some(mapping) = &config.name_mapping {
        if let Some(mapped_name) = mapping.get(&cleaned) {
            info!("📺 频道映射: {} -> {}", cleaned, mapped_name);
            return mapped_name.clone();
        } 
    } 
    
    // 3. 如果没有映射，返回清理后的名称
    debug!("最终名称: {}", cleaned);
    cleaned
}


#[get("/playlist")]
async fn playlist(config: Data<YamlConfig>, _req: HttpRequest) -> impl Responder {
    debug!("Get playlist");
    // let scheme = req.connection_info().scheme().to_owned();
    // let host = req.connection_info().host().to_owned();
    match get_channels(&config.iptv, false).await {
        Err(e) => {
            println!(" playlist: 获取失败 {}", config.iptv.user);
            if let Some(old_playlist) = OLD_PLAYLIST.try_lock().ok().and_then(|f| f.to_owned()) {
                HttpResponse::Ok()
                    .content_type("application/vnd.apple.mpegurl")
                    .body(old_playlist)
            } else {
                HttpResponse::InternalServerError().body(format!("Error getting channels: {}", e))
            }
        }
        Ok(ch) => {

            let m3u_header = if config.m3u8.x_tvg_url.is_empty() {
                String::from("#EXTM3U\n")
            } else {
                format!("#EXTM3U x-tvg-url=\"{}\" \n", config.m3u8.x_tvg_url)
            };
            let playlist = m3u_header 
                + &ch
                    .into_iter()
                    .map(|c| {
                        let group = if c.name.contains("超清") {
                            "超清频道"
                        } else if c.name.contains("高清") {
                            "高清频道"
                        } else {
                            "普通频道"
                        };

                        let tvgname = if config.m3u8.format_tvg {

                            // 直接使用映射或格式化名称
                            format_channel_name(&c.name, &config)
                        } else {
                            c.name.clone()
                        };

                        let tvglogo = format!("https://live.fanmingming.com/tv/{}.png", tvgname);
 

                        let rtsp = if config.m3u8.rtsp_proxy_uri.is_empty() {
                            c.rtsp 
                        } else {
                            c.rtsp.replace("rtsp://", &format!("{}/rtsp/", config.m3u8.rtsp_proxy_uri))
                        };

                        let catch_up = {
                            let connector = if rtsp.contains('?') {
                                "&"
                            } else {
                                "?"
                            };
                            format!(
                                r#" catchup="default" catchup-source="{}{}playseek=${{(b)yyyyMMddHHmmss}}-${{(e)yyyyMMddHHmmss}}" "#,
                                rtsp, connector
                            )
                        };

                        let play_url = if config.m3u8.udp_proxy_uri.is_empty() {
                            c.igmp 
                        } else {
                            c.igmp.replace("igmp://", &format!("{}/udp/", config.m3u8.udp_proxy_uri))
                        };


                        
                        format!(
                            r#"#EXTINF:-1 tvg-id="{id}" tvg-name="{tvgname}" tvg-chno="{chno}" {catch_up} tvg-logo="{tvglogo}" group-title="{group}",{name}"#,
                            id = c.id,
                            chno = c.user_channel_id,
                            name = c.name,
                            group = group,
                            catch_up = catch_up,
                            tvglogo = tvglogo,
                            tvgname = tvgname
                        ) + "\n" + &play_url
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
                + &match &config.m3u8.extra_playlist {
                    Some(u) => parse_extra_playlist(u).await.unwrap_or(String::from("")),
                    None => String::from(""),
                };
            if let Ok(mut old_playlist) = OLD_PLAYLIST.try_lock() {
                *old_playlist = Some(playlist.clone());
            }
            HttpResponse::Ok()
                .content_type("application/vnd.apple.mpegurl")
                .body(playlist)
        }
    }
}

fn mask_password(password: &str) -> String {
    let len = password.len();
    match len {
        0..=4 => password.to_string(),
        5..=8 => {
            // 对于5-8位密码，显示首尾各2位，中间4位用星号
            let start = &password[0..2];
            let end = &password[len-2..];
            format!("{}****{}", start, end)
        },
        _ => {
            // 对于9位及以上密码，显示前4位和后4位，中间用星号
            let start = &password[0..4];
            let end = &password[len-4..];
            let middle_stars = "*".repeat(len - 8);
            format!("{}{}{}", start, middle_stars, end)
        }
    }
}

#[actix_web::main] // or #[tokio::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();

    
    // 解析命令行参数
    let cli_args = match Args::parse() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("❌ 参数解析错误: {}", e);
            Args::usage("iptv");
            std::process::exit(1);
        }
    };

    // 加载 YAML 配置
    let config_path = PathBuf::from(&cli_args.config_file);
    let yaml_config = match YamlConfig::from_file(&config_path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("❌ YAML 配置加载失败: {}", e);
            std::process::exit(1);
        }
    };

    println!("📡 iptv账号:{}  密码:{}", yaml_config.iptv.user, mask_password(&yaml_config.iptv.passwd));

    // 提前获取需要使用的值
    let listen_addr = yaml_config.server.listen.clone();
    let workers = yaml_config.server.workers.unwrap_or(4);

    let server = HttpServer::new(move || {
        let config_data = Data::new(yaml_config.clone());
        App::new()
            .service(xmltv)
            .service(playlist)
            .service(logo)
            .app_data(config_data)
    })
    .workers(workers)
    .bind(&listen_addr)?;
    
    // 获取实际绑定的地址
    let addrs: Vec<std::net::SocketAddr> = server.addrs();
    for addr in &addrs {
        println!("✅ 服务已启动: http://{}", addr);
        println!("📺 XMLTV 地址: http://{}/xmltv", addr);
        println!("📋 播放列表地址: http://{}/playlist", addr);
        println!("🖼️ Logo 地址: http://{}/logo", addr);
    }
    
    
    server.run().await
}