#!/usr/bin/env bash
# 开发环境载入脚本（macOS / Linux）
#
#     . ./scripts/dev.sh
#
# Windows 用 scripts/dev.ps1（它还会导入 MSVC 的 vcvars 环境）。

set -euo pipefail

export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
case ":$PATH:" in
  *":$CARGO_HOME/bin:"*) ;;
  *) export PATH="$CARGO_HOME/bin:$PATH" ;;
esac

if ! command -v cargo >/dev/null 2>&1; then
  echo "未找到 cargo，请先安装 Rust：https://rustup.rs" >&2
  return 1 2>/dev/null || exit 1
fi

# 国内网络建议保留工程内 .cargo/config.toml 的 rsproxy 镜像；
# 在海外可删除该文件，直接走 crates.io。
if [ -f ".cargo/config.toml" ]; then
  echo "cargo 镜像：工程内 .cargo/config.toml（rsproxy）"
fi

echo "Rust: $(rustc --version)"
echo "Target: $(rustc -vV | awk '/^host:/ {print $2}')"

case "$(uname -s)" in
  Linux)
    if ! pkg-config --exists libudev 2>/dev/null; then
      echo "提示：Linux 上串口枚举需要 libudev 开发包，例如" >&2
      echo "  Debian/Ubuntu: sudo apt install libudev-dev" >&2
      echo "  Fedora:        sudo dnf install systemd-devel" >&2
    fi
    ;;
esac
