use serde::Deserialize;
use std::path::PathBuf;
use std::collections::HashMap;

/// 主配置文件结构
#[derive(Debug, Deserialize, Clone)]
pub struct YamlConfig {
    pub server: ServerConfig,
    pub iptv: IptvConfig,
    pub m3u8: M3u8Config,
    pub name_mapping: Option<HashMap<String, String>>,
    #[serde(default)]  // 允许该字段不存在
    pub name_clean: Vec<String>,  // 直接是字符串数组，不是嵌套结构
}

/// 服务器配置
#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub listen: String,
    pub workers: Option<usize>,
    pub timeout: Option<u64>,
    pub log_level: Option<String>,
}

/// IPTV 认证配置
#[derive(Debug, Deserialize, Clone)]
pub struct IptvConfig {
    pub user: String,
    pub passwd: String,
    pub mac: String,
    pub imei: Option<String>,
    pub ip: Option<String>,
    pub interface: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct M3u8Config {
    #[serde(default)]
    pub x_tvg_url: String,
    
    #[serde(default)]
    pub format_tvg: bool,
    
    pub extra_playlist: Option<String>,
    pub extra_xmltv: Option<String>,
    
    #[serde(default)]
    pub udp_proxy_uri: String,
    
    #[serde(default)]
    pub rtsp_proxy_uri: String,
}


impl YamlConfig {
    /// 从文件加载配置
    pub fn from_file(path: &PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let config_content = std::fs::read_to_string(path)?;
        let config: YamlConfig = serde_yaml::from_str(&config_content)?;
        Ok(config)
    }
}