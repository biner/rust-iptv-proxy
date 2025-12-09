use actix_web::{
    get,
    web::{Data, Path, Query},
    HttpRequest, HttpResponse, Responder,
};

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use once_cell::sync::Lazy;

use log::{debug, info, error};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;  // 添加这行
// use xml::EventReader;  // 删除这行

use crate::config::YamlConfig;
use crate::iptv::{get_channels, get_icon,  get_channels_epg, get_channel_date_epg, Channel};
use crate::utils::{
    to_xmltv, parse_extra_xml, parse_extra_playlist, 
    format_channel_name, timestamp_to_hhmm
};


// 全局状态
static OLD_PLAYLIST: Mutex<Option<String>> = Mutex::new(None);
static OLD_XMLTV: Mutex<Option<String>> = Mutex::new(None);


static TVGNAME_MAPPING: Lazy<Arc<RwLock<HashMap<String, u64>>>> = Lazy::new(|| {
    Arc::new(RwLock::new(HashMap::new()))
});

// 全局 channel_id -> tvgname 反向映射
static ID_TO_TVGNAME: Lazy<Arc<RwLock<HashMap<u64, String>>>> = Lazy::new(|| {
    Arc::new(RwLock::new(HashMap::new()))
});


// 查询参数结构
#[derive(Debug, Deserialize)]
pub struct EpgQuery {
    pub ch: String,   // 频道名称
    pub date: String, // 日期，格式如 20251202
    pub id: Option<u64>,
}

// 响应结构
#[derive(Debug, Serialize)]
pub struct EpgResponse {
    pub date: String,
    pub channel_name: String,
    pub url: String,
    pub epg_data: Vec<EpgItem>,
}

#[derive(Debug, Serialize)]
pub struct EpgItem {
    pub start: String,
    pub end: String,
    pub title: String,
}

#[get("/xmltv")]
pub async fn xmltv(config: Data<YamlConfig>, _req: HttpRequest) -> impl Responder {
    debug!("Get EPG");
    
    // 获取额外的 XML 内容
    let extra_xml = match &config.m3u8.extra_xmltv {
        Some(u) => parse_extra_xml(u).await.ok(),
        None => None,
    };
    
    let xml = get_channels_epg(&config.iptv)
        .await
        .and_then(|ch| to_xmltv(ch, extra_xml));  // 现在传递 String 而不是 EventReader
    
    match xml {
        Err(e) => {
            if let Some(old_xmltv) = OLD_XMLTV.try_lock().ok().and_then(|f| f.to_owned()) {
                HttpResponse::Ok().content_type("text/xml").body(old_xmltv)
            } else {
                HttpResponse::InternalServerError().body(format!("Error getting channels: {}", e))
            }
        }
        Ok(xml) => {
            // 缓存最新的 XMLTV
            if let Ok(mut old_xmltv) = OLD_XMLTV.try_lock() {
                *old_xmltv = Some(xml.clone());
            }
            HttpResponse::Ok().content_type("text/xml").body(xml)
        }
    }
}


/// 更新全局映射
fn update_global_mappings(
    tvgname_to_id: HashMap<String, u64>,
    id_to_tvgname: HashMap<u64, String>,
) {
    // 更新 tvgname -> id 映射
    if let Ok(mut mapping) = TVGNAME_MAPPING.write() {
        mapping.clear();
        mapping.extend(tvgname_to_id);
    }
    
    // 更新 id -> tvgname 反向映射
    if let Ok(mut reverse_mapping) = ID_TO_TVGNAME.write() {
        reverse_mapping.clear();
        reverse_mapping.extend(id_to_tvgname);
    }
}

/// 根据 tvgname 获取 channel_id
pub fn get_channel_id_by_tvgname(tvgname: &str) -> Option<u64> {
    TVGNAME_MAPPING.read().ok()
        .and_then(|mapping| mapping.get(tvgname).copied())
}

// /// 根据 channel_id 获取 tvgname
// pub fn get_tvgname_by_channel_id(channel_id: u64) -> Option<String> {
//     ID_TO_TVGNAME.read().ok()
//         .and_then(|mapping| mapping.get(&channel_id).cloned())
// }

// /// 获取所有 tvgname -> id 映射
// pub fn get_all_tvgname_mapping() -> HashMap<String, u64> {
//     TVGNAME_MAPPING.read().ok()
//         .map(|mapping| mapping.clone())
//         .unwrap_or_default()
// }

// /// 获取所有 id -> tvgname 映射
// pub fn get_all_id_mapping() -> HashMap<u64, String> {
//     ID_TO_TVGNAME.read().ok()
//         .map(|mapping| mapping.clone())
//         .unwrap_or_default()
// }

// /// 获取映射数量
// pub fn get_tvgname_mapping_count() -> usize {
//     TVGNAME_MAPPING.read().ok()
//         .map(|mapping| mapping.len())
//         .unwrap_or(0)
// }

// /// 检查 tvgname 是否存在
// pub fn has_tvgname(tvgname: &str) -> bool {
//     TVGNAME_MAPPING.read().ok()
//         .map(|mapping| mapping.contains_key(tvgname))
//         .unwrap_or(false)
// }

// /// 搜索 tvgname（支持模糊搜索）
// pub fn search_tvgname(keyword: &str) -> Vec<(String, u64)> {
//     TVGNAME_MAPPING.read().ok()
//         .map(|mapping| {
//             mapping.iter()
//                 .filter(|(tvgname, _)| {
//                     tvgname.to_lowercase().contains(&keyword.to_lowercase())
//                 })
//                 .map(|(tvgname, &id)| (tvgname.clone(), id))
//                 .collect()
//         })
//         .unwrap_or_default()
// }

#[get("/playlist")]
pub async fn playlist(config: Data<YamlConfig>, _req: HttpRequest) -> impl Responder {
    debug!("Get playlist");
    
    match get_channels(&config.iptv).await {
        Err(e) => {
            info!("playlist: 获取失败 {}", config.iptv.user);
            if let Some(old_playlist) = OLD_PLAYLIST.try_lock().ok().and_then(|f| f.to_owned()) {
                HttpResponse::Ok()
                    .content_type("application/vnd.apple.mpegurl")
                    .body(old_playlist)
            } else {
                HttpResponse::InternalServerError().body(format!("Error getting channels: {}", e))
            }
        }
        Ok(ch) => {

            // 1. 创建本地映射
            let mut tvgname_to_id = HashMap::new();
            let mut id_to_tvgname = HashMap::new();
            
            for channel in &ch {
                let tvgname = if config.m3u8.format_tvg {
                    // format_channel_name(
                    //     &channel.name, 
                    //     config.name_mapping.as_ref(),
                    //     &config.name_clean
                    // )
                    channel.name.clone()
                } else {
                    channel.name.clone()
                };
                info!("tvgname: {}", tvgname);
                
                tvgname_to_id.insert(tvgname.clone(), channel.id);
                id_to_tvgname.insert(channel.id, tvgname);
            }
            
            // 2. 更新全局映射
            update_global_mappings(tvgname_to_id, id_to_tvgname);
            


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
                            format_channel_name(
                                &c.name, 
                                config.name_mapping.as_ref(),  // 传递 Option<&HashMap>
                                &config.name_clean             // 传递 &[String]
                            )
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
            
            // 缓存播放列表
            if let Ok(mut old_playlist) = OLD_PLAYLIST.try_lock() {
                *old_playlist = Some(playlist.clone());
            }
            
            HttpResponse::Ok()
                .content_type("application/vnd.apple.mpegurl")
                .body(playlist)
        }
    }
}

#[get("/logo/{id}.png")]
pub async fn logo(config: Data<YamlConfig>, path: Path<String>) -> impl Responder {
    debug!("Get logo");
    match get_icon(&config.iptv, &path).await {
        Ok(icon) => HttpResponse::Ok().content_type("image/png").body(icon),
        Err(e) => HttpResponse::NotFound().body(format!("Error getting channels: {}", e)),
    }
}

#[get("/")]
pub async fn epg(
    config: Data<YamlConfig>,
    query: Query<EpgQuery>,
) -> impl Responder {
    debug!("EPG API 请求: ch={}, date={}", query.ch, query.date);
    
    // 1. 转换日期格式
    let date_str = format_date_string(&query.date).unwrap_or_else(|| query.date.clone());
    
    // 2. 登录获取频道列表，找到对应的频道ID
    let channel_id = if let Some(id) = query.id {
        debug!("使用提供的频道ID: {}", id);
        id
    } else {
        // 2. 尝试通过 tvgname 查找
        match get_channel_id_by_tvgname(&query.ch) {
            Some(id) => {
                debug!("通过全局映射找到频道: {} -> ID: {}", query.ch, id);
                id
            }
            None => {
                // 3. 如果全局映射中没有，尝试常见映射
                error!("无法找到频道: {}", &query.ch);
                return HttpResponse::NotFound()
                    .json(serde_json::json!({
                        "error": format!("频道 '{}' 未找到", query.ch),
                    }));
            }
        }
    };
        
        // 3. 获取该频道的完整数据（包括频道信息和 EPG）
    let channel_data = match get_channel_date_epg(&config.iptv, channel_id, &date_str).await {
        Ok(channel) => channel,
        Err(e) => {
            error!("获取频道 {} 的 EPG 数据失败: {}", channel_id, e);
            
            // 如果获取失败，返回基本的频道信息（无 EPG）
            // 首先尝试从映射中获取频道名称，如果失败则使用查询的频道名
            let channel_name = query.ch.clone();
            Channel {
                id: channel_id,
                name: channel_name,
                user_channel_id: "0".to_string(),
                rtsp: "".to_string(),
                igmp: "".to_string(),
                epg: Vec::new(),
            }
        }
    };
        
    // 4. 转换时间格式（毫秒时间戳 -> HH:MM）
    let epg_items: Vec<EpgItem> = channel_data.epg.into_iter()
        .map(|program| {
            EpgItem {
                start: timestamp_to_hhmm(program.start),
                end: timestamp_to_hhmm(program.stop),
                title: program.title,
            }
        })
        .collect();
        
    // 5. 构建响应
    let response = EpgResponse {
        date: date_str,
        channel_name: channel_data.name,  // 使用实际获取的频道名称
        url: channel_data.igmp,  // 或者 channel_data.rtsp / channel_data.igmp
        epg_data: epg_items,
    };
        
    HttpResponse::Ok()
        .content_type("application/json")
        .json(response)

}

/// 转换日期格式：20251202 -> 2025-12-02
fn format_date_string(date_str: &str) -> Option<String> {
    if date_str.len() == 8 {
        // 格式：YYYYMMDD
        let year = &date_str[0..4];
        let month = &date_str[4..6];
        let day = &date_str[6..8];
        
        // 验证月份和日期是否有效
        if let Ok(month_num) = month.parse::<u32>() {
            if (1..=12).contains(&month_num) {
                if let Ok(day_num) = day.parse::<u32>() {
                    if (1..=31).contains(&day_num) {
                        return Some(format!("{}-{}-{}", year, month, day));
                    }
                }
            }
        }
    }
    
    None
}

