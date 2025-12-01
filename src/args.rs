use argh::FromArgs;

/// IPTV 代理服务器命令行参数
#[derive(FromArgs, Clone)]
pub struct Args {
    /// 配置文件路径
    #[argh(option, short = 'c')]
    pub config_file: String,


}

impl Args {
    /// 解析命令行参数
    pub fn parse() -> Result<Self, String> {
        let args_vec = std::env::args().collect::<Vec<_>>();
        let args_vec = args_vec.iter().map(|s| s.as_str()).collect::<Vec<_>>();
        let args_slice: &[&str] = args_vec.as_ref();
        
        if args_slice.is_empty() {
            return Err("无法解析命令行参数".to_string());
        }
        
        match Self::from_args(&args_slice[0..1], &args_slice[1..]) {
            Ok(args) => Ok(args),
            Err(output) => {
                if output.status.is_ok() {
                    // 显示帮助信息后退出
                    println!("{}", output.output);
                    std::process::exit(0);
                } else {
                    Err(output.output)
                }
            }
        }
    }

    /// 显示用法信息
    pub fn usage(program_name: &str) {
        let usage = format!(
            r#"Usage: {} [OPTIONS] --user <USER> --passwd <PASSWD> --mac <MAC>

    Options:
        -u, --user <USER>                      Login username
        -p, --passwd <PASSWD>                  Login password
        -m, --mac <MAC>                        MAC address
        -i, --imei <IMEI>                      IMEI [default: ]
        -b, --bind <BIND>                      Bind address:port [default: 0.0.0.0:7878]
        -a, --address <ADDRESS>                IP address/interface name [default: ]
        -I, --interface <INTERFACE>            Interface to request
            --x-tvg-url <X_TVG_URL>            使用m3u中的x-tvg-url字段[exmaple:http://172.18.0.19:7878/xmltv]
            --format-tvg  <FORMAT_TVG>         格式化tvgname字段,可以通过 tvg-name 识别频道节目, 例如: CCTV1高清 -> CCTV1
            --extra-playlist <EXTRA_PLAYLIST>  Url to extra m3u
            --extra-xmltv <EXTRA_XMLTV>        Url to extra xmltv
            --udp-proxy                        Use UDP proxy
            --udp-proxy-uri <UDP_PROXY_URI>    第三方(udpxy,msd_lite,rtp2httpd)提供的UDP代理URI[exmaple:http://192.168.1.1:5146]
            --rtsp-proxy                       Use rtsp proxy
            --rtsp-proxy-uri <RTSP_PROXY_URI>  第三方(rtp2httpd)提供的rtsp代理URI[exmaple:http://192.168.1.1:5146]
        -h, --help                             Print help
    "#,
            program_name
        );
        eprint!("{}", usage);

    }



    /// 验证必需参数
    pub fn validate(&self) -> Result<(), String> {

        Ok(())
    }
}