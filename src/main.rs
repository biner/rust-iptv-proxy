use actix_web::{
    web::Data,
    App, HttpServer,
};

use std::path::PathBuf;


mod args;
mod config;
mod iptv;
mod routes;
mod utils;

use args::Args;
use config::YamlConfig;
use utils::mask_password;



fn init_logger_simple(config: &YamlConfig) {
    if std::env::var("RUST_LOG").is_err() {
        let log_level = config.server.log_level.clone();
        // 原代码（第24行）：
        // std::env::set_var("RUST_LOG", log_level);
        
        // 修复后的代码：
        unsafe {
            std::env::set_var("RUST_LOG", log_level);
        }
    }
    
    env_logger::init();
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let cli_args = match Args::parse() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("❌ 参数解析错误: {}", e);
            Args::usage("iptv");
            std::process::exit(1);
        }
    };

    let config_path = PathBuf::from(&cli_args.config_file);
    let yaml_config = match YamlConfig::from_file(&config_path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("❌ YAML 配置加载失败: {}", e);
            std::process::exit(1);
        }
    };

    init_logger_simple(&yaml_config);

    println!("📡 iptv账号:{}  密码:{}", 
        yaml_config.iptv.user, 
        mask_password(&yaml_config.iptv.passwd)
    );

    let listen_addr = yaml_config.server.listen.clone();
    let workers = yaml_config.server.workers;

    let server = HttpServer::new(move || {
        let config_data = Data::new(yaml_config.clone());
        App::new()
            .service(routes::xmltv)
            .service(routes::playlist)
            .service(routes::logo)
            .service(routes::epg)
            .app_data(config_data)
    })
    .workers(workers)
    .bind(&listen_addr)?;
    
    let addrs: Vec<std::net::SocketAddr> = server.addrs();
    for addr in &addrs {
        println!("✅ 服务已启动: http://{}", addr);
        println!("📺 XMLTV 地址: http://{}/xmltv", addr);
        println!("📋 播放列表地址: http://{}/playlist", addr);
        println!("🖼️ Logo 地址: http://{}/logo", addr);
    }
    
    server.run().await
}