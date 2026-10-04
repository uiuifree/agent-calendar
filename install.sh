#!/bin/sh
# agent-calendar installer: downloads the latest release binary into ~/.local/bin (or $AGENT_CALENDAR_INSTALL_DIR).
#   curl -fsSL https://raw.githubusercontent.com/uiuifree/agent-calendar/main/install.sh | sh
set -eu

repo="uiuifree/agent-calendar"
dir="${AGENT_CALENDAR_INSTALL_DIR:-$HOME/.local/bin}"

case "$(uname -s)" in
  Linux) os="unknown-linux-gnu" ;;
  Darwin) os="apple-darwin" ;;
  *) echo "unsupported OS: $(uname -s) (Linux, WSL2 and macOS are supported)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  aarch64 | arm64) arch="aarch64" ;;
  *) echo "unsupported CPU: $(uname -m)" >&2; exit 1 ;;
esac

target="$arch-$os"
url="https://github.com/$repo/releases/latest/download/agent-calendar-$target.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -r "$tmp"' EXIT

echo "downloading $url"
if ! curl -fsSL "$url" -o "$tmp/agent-calendar.tar.gz"; then
  echo "error: could not download a release for $target." >&2
  echo "If no release has been published yet, build from source instead:" >&2
  echo "  git clone https://github.com/$repo && cd agent-calendar" >&2
  echo "  (cd web && npm ci && npm run build) && cargo install --path ." >&2
  exit 1
fi
tar xzf "$tmp/agent-calendar.tar.gz" -C "$tmp"
mkdir -p "$dir"
install -m 755 "$tmp/agent-calendar-$target/agent-calendar" "$dir/agent-calendar"
echo "installed $dir/agent-calendar"

case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "note: $dir is not on your PATH" ;;
esac

if [ "$os" = "unknown-linux-gnu" ] && command -v systemctl >/dev/null 2>&1; then
  echo "next: run 'agent-calendar service install' to start it in the background, then open http://127.0.0.1:8082/"
else
  echo "next: run 'agent-calendar serve' and open http://127.0.0.1:8082/"
fi
