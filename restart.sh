#!/bin/sh
echo "重启 IPTV 服务..."
pkill -f "iptv" || true
# sleep 1
export RUST_LOG=info
cargo run -- \
    --config-file /application/dev.yaml