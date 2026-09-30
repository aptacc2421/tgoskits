#!/bin/sh
# 一次性：把 v6 的最新版（Windows 侧产出，因 9P 无法覆盖已有文件而暂存为 *.new）
# 替换到本目录。在 WSL 里执行：
#   sh os/axvisor/web-ui/design/v6/update.sh
# 跑完确认后可删除 *.new 与本脚本。
set -e
cd "$(dirname "$0")"
for f in index.html style.css app.js preview.html; do
  cp "$f.new" "$f" && echo "updated $f"
done
cp shots/s07.png.new shots/s07.png && echo "updated shots/s07.png"
cp shots/s08.png.new shots/s08.png && echo "updated shots/s08.png"
rm -f index.html.new style.css.new app.js.new preview.html.new shots/s07.png.new shots/s08.png.new update.sh
echo "done"
