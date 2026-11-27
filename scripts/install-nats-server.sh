#!/usr/bin/env bash
# Download a nats-server release and print the directory holding the binary.
#   ./scripts/install-nats-server.sh v2.15.3 /tmp/nats >> "$GITHUB_PATH"
set -euo pipefail

version="${1:?usage: $0 VERSION DEST}"
dest="${2:?usage: $0 VERSION DEST}"

case "$(uname -s)" in
  Linux) os=linux ext=tar.gz ;;
  Darwin) os=darwin ext=tar.gz ;;
  MINGW*|MSYS*|CYGWIN*) os=windows ext=zip ;;
  *) echo "unsupported OS $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64|amd64) arch=amd64 ;;
  arm64|aarch64) arch=arm64 ;;
  *) echo "unsupported arch $(uname -m)" >&2; exit 1 ;;
esac

name="nats-server-${version}-${os}-${arch}"
url="https://github.com/nats-io/nats-server/releases/download/${version}/${name}.${ext}"
mkdir -p "$dest"
echo "downloading $url" >&2
curl -fsSL --retry 3 -o "$dest/$name.$ext" "$url"
if [ "$ext" = zip ]; then
  unzip -q -o "$dest/$name.$ext" -d "$dest"
else
  tar -xzf "$dest/$name.$ext" -C "$dest"
fi
dir="$dest/$name"
if command -v cygpath >/dev/null 2>&1; then
  cygpath -w "$dir"
else
  echo "$dir"
fi
