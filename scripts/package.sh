#!/usr/bin/env bash
# Build a release binary for TARGET and package it into OUT:
#   lictor-<version>-<target>.tar.gz (or .zip on Windows), plus .deb and .rpm for Linux targets,
#   each with a .sha256 file.
#   ./scripts/package.sh x86_64-unknown-linux-musl dist
# Linux targets are cross-compiled with cargo-zigbuild; others use plain cargo.
set -euo pipefail

target="${1:?usage: $0 TARGET OUTDIR}"
out="${2:?usage: $0 TARGET OUTDIR}"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
mkdir -p "$out"

case "$target" in
  *-linux-*) cargo zigbuild --release --locked -p lictor --target "$target" ;;
  *) cargo build --release --locked -p lictor --target "$target" ;;
esac

bin="target/$target/release/lictor"
[ -f "$bin.exe" ] && bin="$bin.exe"
stage="$(mktemp -d)/lictor-$version-$target"
mkdir -p "$stage"
cp "$bin" README.md LICENSE "$stage/"

sha() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi
}

case "$target" in
  *-windows-*)
    archive="$out/lictor-$version-$target.zip"
    (cd "$(dirname "$stage")" && 7z a -tzip -bso0 "$OLDPWD/$archive" "$(basename "$stage")")
    ;;
  *)
    archive="$out/lictor-$version-$target.tar.gz"
    tar -czf "$archive" -C "$(dirname "$stage")" "$(basename "$stage")"
    ;;
esac
(cd "$out" && sha "$(basename "$archive")" > "$(basename "$archive").sha256")

case "$target" in
  *-linux-*)
    cargo deb -p lictor --no-build --no-strip --target "$target" --output "$out/"
    cargo generate-rpm -p crates/lictor --target "$target" -o "$out/"
    for f in "$out"/*.deb "$out"/*.rpm; do
      (cd "$out" && sha "$(basename "$f")" > "$(basename "$f").sha256")
    done
    ;;
esac
ls -l "$out"
