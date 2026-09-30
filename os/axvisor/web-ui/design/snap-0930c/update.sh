#!/bin/sh
# 一次性：把 v6 的最新版（本目录，Windows 侧产出）覆盖到 ../v6/。
# 背景：9P 路径上 Windows 侧无法覆盖 WSL 中已存在的文件，所以最新快照放在
# v6-latest/，由本脚本在 WSL 内完成替换。
# 执行：
#   sh os/axvisor/web-ui/design/v6-latest/update.sh
# 确认后可整体删除 v6-latest/ 与 v6/ 里遗留的 *.new。
set -e
cd "$(dirname "$0")"
V6="$(dirname "$PWD")/v6"
for f in index.html style.css app.js preview.html; do
  cp "$f" "$V6/$f" && echo "updated v6/$f"
done
mkdir -p "$V6/shots"
cp shots/*.png "$V6/shots/" && echo "updated v6/shots"
echo "done"
