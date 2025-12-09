#!/bin/sh
echo "重启 IPTV 服务..."
pkill -f "iptv" || true

# 设置环境变量
export RUST_LOG=info


# 创建日志目录

cd /application
cargo run --   \
    --config-file /application/dev.yaml  \
# target/debug/iptv   \
#     --config-file /application/config.yaml  \


# cargo build --release
# target/release/iptv   \
#     --config-file /application/config.yaml  