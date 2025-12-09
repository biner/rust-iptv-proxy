// utils.rs
use anyhow::{anyhow, Result};
use chrono::{FixedOffset, TimeZone, Utc};
use log::{debug, info};
use quick_xml::{
    events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event},
    Reader, Writer,
};
use reqwest::Client;
use std::io::{BufWriter};
use std::collections::HashMap;

use chrono::Duration as ChronoDuration;
use crate::iptv::Channel;


/// 将时间戳（毫秒）转换为 HH:MM 格式
pub fn timestamp_to_hhmm(timestamp: i64) -> String {
    // 时间戳可能是毫秒，先转换为秒
    let timestamp_seconds = timestamp / 1000;
    
    // 使用 chrono 转换
    if let Some(datetime) = Utc.timestamp_opt(timestamp_seconds, 0).single() {

        let cst_datetime = datetime + ChronoDuration::hours(8);
        cst_datetime.format("%H:%M").to_string()
    } else {
        // 如果转换失败，返回原始值或空字符串
        "00:00".to_string()
    }
}


// XMLTV 时间转换函数
pub fn to_xmltv_time(unix_time: i64) -> Result<String> {
    match Utc.timestamp_millis_opt(unix_time) {
        chrono::LocalResult::Single(t) => Ok(t
            .with_timezone(&FixedOffset::east_opt(8 * 60 * 60).ok_or(anyhow!(""))?)
            .format("%Y%m%d%H%M%S")
            .to_string()),
        _ => Err(anyhow!("fail to parse time")),
    }
}

// 生成 XMLTV 的函数
pub fn to_xmltv(channels: Vec<Channel>, extra_xml: Option<String>) -> Result<String> {
    let mut writer = Writer::new(BufWriter::new(Vec::new()));
    
    // 写入 XML 声明 - 使用 BytesDecl
    let decl = BytesDecl::new("1.0", Some("UTF-8"), None);
    writer.write_event(Event::Decl(decl))?;
    
    // 开始 tv 元素
    let mut tv_elem = BytesStart::new("tv");
    tv_elem.push_attribute(("generator-info-name", "iptv-proxy"));
    tv_elem.push_attribute(("source-info-name", "iptv-proxy"));
    writer.write_event(Event::Start(tv_elem))?;
    
    // 写入频道信息
    for channel in channels.iter() {
        let mut channel_elem = BytesStart::new("channel");
        channel_elem.push_attribute(("id", channel.id.to_string().as_str()));
        writer.write_event(Event::Start(channel_elem))?;
        
        writer.write_event(Event::Start(BytesStart::new("display-name")))?;
        writer.write_event(Event::Text(BytesText::new(&channel.name)))?;
        writer.write_event(Event::End(BytesEnd::new("display-name")))?;
        
        writer.write_event(Event::End(BytesEnd::new("channel")))?;
    }
    
    // 如果有额外的 XML 内容，合并进来
    if let Some(extra) = extra_xml {
        let mut reader = Reader::from_str(&extra);
        reader.config_mut().trim_text(true);
        
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let name = e.name();
                    let name_str = std::str::from_utf8(name.as_ref())?;
                    
                    // 只复制需要的元素
                    if name_str == "channel" 
                        || name_str == "display-name" 
                        || name_str == "desc" 
                        || name_str == "title" 
                        || name_str == "sub-title" 
                        || name_str == "programme" {
                        
                        // 检查 title 元素的 lang 属性
                        let mut should_write = true;
                        let mut elem = BytesStart::new(name_str);
                        
                        if name_str == "title" {
                            for attr in e.attributes() {
                                let attr = attr?;
                                let key = std::str::from_utf8(attr.key.as_ref())?;
                                let value = std::str::from_utf8(&attr.value)?;
                                
                                if key == "lang" && value != "chi" {
                                    should_write = false;
                                    break;
                                }
                                elem.push_attribute((key, value));
                            }
                        } else {
                            for attr in e.attributes() {
                                let attr = attr?;
                                let key = std::str::from_utf8(attr.key.as_ref())?;
                                let value = std::str::from_utf8(&attr.value)?;
                                elem.push_attribute((key, value));
                            }
                        }
                        
                        if should_write {
                            writer.write_event(Event::Start(elem))?;
                        }
                    }
                }
                Ok(Event::Text(e)) => {
                    writer.write_event(Event::Text(e))?;
                }
                Ok(Event::End(e)) => {
                    let name = e.name();
                    let name_str = std::str::from_utf8(name.as_ref())?;
                    
                    if name_str == "channel" 
                        || name_str == "display-name" 
                        || name_str == "desc" 
                        || name_str == "title" 
                        || name_str == "sub-title" 
                        || name_str == "programme" {
                        writer.write_event(Event::End(e))?;
                    }
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }
    }
    
    // 写入节目信息
    for channel in channels.iter() {
        for epg in channel.epg.iter() {
            let mut prog_elem = BytesStart::new("programme");
            prog_elem.push_attribute(("start", format!("{} +0800", to_xmltv_time(epg.start)?).as_str()));
            prog_elem.push_attribute(("stop", format!("{} +0800", to_xmltv_time(epg.stop)?).as_str()));
            prog_elem.push_attribute(("channel", channel.id.to_string().as_str()));
            writer.write_event(Event::Start(prog_elem))?;
            
            // 标题
            let mut title_elem = BytesStart::new("title");
            title_elem.push_attribute(("lang", "chi"));
            writer.write_event(Event::Start(title_elem))?;
            writer.write_event(Event::Text(BytesText::new(&epg.title)))?;
            writer.write_event(Event::End(BytesEnd::new("title")))?;
            
            // 描述（如果有）
            if !epg.desc.is_empty() {
                writer.write_event(Event::Start(BytesStart::new("desc")))?;
                writer.write_event(Event::Text(BytesText::new(&epg.desc)))?;
                writer.write_event(Event::End(BytesEnd::new("desc")))?;
            }
            
            writer.write_event(Event::End(BytesEnd::new("programme")))?;
        }
    }
    
    // 结束 tv 元素
    writer.write_event(Event::End(BytesEnd::new("tv")))?;
    
    let result = writer.into_inner().into_inner()?;
    Ok(String::from_utf8(result)?)
}

// 修改 parse_extra_xml 函数
pub async fn parse_extra_xml(url: &str) -> Result<String> {
    let client = Client::builder().build()?;
    let url = reqwest::Url::parse(url)?;
    let response = client.get(url).send().await?.error_for_status()?;
    Ok(response.text().await?)
}

// 其他函数保持不变...
pub async fn parse_extra_playlist(url: &str) -> Result<String> {
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

/// 格式化频道名称（带详细日志，包含清理功能）
pub fn format_channel_name(
    name: &str, 
    name_mapping: Option<&HashMap<String, String>>,
    name_clean: &[String]
) -> String {
    debug!("开始处理频道名称: {}", name);
    
    // 1. 先应用清理规则
    let mut cleaned = name.to_string();
    
    // 创建可变的清理模式副本，并按长度排序
    let mut patterns = name_clean.to_vec();
    patterns.sort_by(|a, b| b.len().cmp(&a.len()));
    patterns.dedup();
    
    debug!("清理模式（按长度排序）: {:?}", patterns);
    
    // 应用所有清理模式
    for pattern in &patterns {
        let original = cleaned.clone();
        cleaned = cleaned.replace(pattern, "");
        if cleaned != original {
            info!("移除 '{}': {} -> {}", pattern, original, cleaned);
        }
    }
    
    // 去除首尾空白字符
    cleaned = cleaned.trim().to_string();
    
    // 2. 应用名称映射（如果有）
    if let Some(mapping) = name_mapping {
        if let Some(mapped_name) = mapping.get(&cleaned) {
            info!("📺 频道映射: {} -> {}", cleaned, mapped_name);
            return mapped_name.clone();
        } 
    } 
    
    // 3. 如果没有映射，返回清理后的名称
    debug!("最终名称: {}", cleaned);
    cleaned
}


pub fn mask_password(password: &str) -> String {
    let len = password.len();
    match len {
        0..=4 => password.to_string(),
        5..=8 => {
            let start = &password[0..2];
            let end = &password[len-2..];
            format!("{}****{}", start, end)
        },
        _ => {
            let start = &password[0..4];
            let end = &password[len-4..];
            let middle_stars = "*".repeat(len - 8);
            format!("{}{}{}", start, middle_stars, end)
        }
    }
}