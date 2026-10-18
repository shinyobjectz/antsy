#!/bin/sh
# Installs the antsy binary to ~/.antsy/bin: a release download if one exists for this machine,
# else a build from this checkout (needs cargo). Fails fast and says why.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
dest="${ANTSY_HOME:-$HOME/.antsy}/bin"
mkdir -p "$dest"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Linux-x86_64) target=x86_64-unknown-linux-musl ;;
  Linux-aarch64|Linux-arm64) target=aarch64-unknown-linux-musl ;;
  *) target="" ;;
esac
url="https://github.com/shinyobjectz/antsy/releases/download/v$version/antsy-$target.tar.gz"
if [ -n "$target" ] && command -v curl >/dev/null && curl -fsSL --max-time 60 "$url" -o "$dest/antsy.tgz" 2>/dev/null; then
  tar -xzf "$dest/antsy.tgz" -C "$dest" && rm -f "$dest/antsy.tgz"
  echo "antsy $version installed from release to $dest/antsy"
elif command -v cargo >/dev/null; then
  echo "no release binary for ${target:-this platform}; building from source (one small crate, three deps)"
  CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo build --release --locked --manifest-path "$root/Cargo.toml"
  cp "$root/target/release/antsy" "$dest/antsy"
  echo "antsy $version built and installed to $dest/antsy"
else
  echo "antsy: no release binary for this platform and no cargo to build one. Install Rust (https://rustup.rs) and rerun." >&2
  exit 1
fi
"$dest/antsy" doctor
