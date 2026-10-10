#!/usr/bin/env bash
set -euo pipefail
project_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
binary="$project_root/build/bin/mulitgit-desktop"
if [[ ! -f "$binary" ]]; then
  echo '请先运行 npm run build:desktop（Linux WebKitGTK 4.1 加上 -- -tags webkit2_41）' >&2
  exit 1
fi
if [[ $(uname -s) != Linux ]]; then
  echo '此安装脚本适用于 Linux；macOS 和 Windows 请使用 Wails 生成的应用包。' >&2
  exit 1
fi
install_dir="$HOME/.local/bin"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
install -Dm755 "$binary" "$install_dir/mulitgit-desktop"
install -Dm644 "$project_root/build/appicon.png" "$data_dir/icons/hicolor/512x512/apps/mulitgit.png"
mkdir -p "$data_dir/applications"
cat > "$data_dir/applications/mulitgit-desktop.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=MulitGit
Comment=多仓库 Git 与 AI 代码审查工作台
Exec="$install_dir/mulitgit-desktop"
Icon=mulitgit
Terminal=false
Categories=Development;RevisionControl;
StartupWMClass=MulitGit
DESKTOP
if command -v update-desktop-database >/dev/null; then update-desktop-database "$data_dir/applications"; fi
printf '已安装：%s\n应用菜单中搜索 MulitGit 即可打开。\n' "$install_dir/mulitgit-desktop"
