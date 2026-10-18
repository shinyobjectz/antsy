#!/bin/sh
# Installs the antsy binary to ~/.antsy/bin: a release download if one exists for this machine,
# else a build with cargo. Works from a checkout or piped: curl -fsSL …/install.sh | sh
# Fails fast and says why.
set -eu
root=$(cd "$(dirname "$0")/.." 2>/dev/null && pwd || echo "")
dest="${ANTSY_HOME:-$HOME/.antsy}/bin"
mkdir -p "$dest"
version=""
if [ -n "$root" ] && [ -f "$root/Cargo.toml" ] && grep -q '^name = "antsy"' "$root/Cargo.toml"; then
  version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
else
  root=""
fi
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Linux-x86_64) target=x86_64-unknown-linux-musl ;;
  Linux-aarch64|Linux-arm64) target=aarch64-unknown-linux-musl ;;
  *) target="" ;;
esac
base=https://github.com/shinyobjectz/antsy/releases
if [ -n "$version" ]; then url="$base/download/v$version/antsy-$target.tar.gz"; else url="$base/latest/download/antsy-$target.tar.gz"; fi
if [ -n "$target" ] && command -v curl >/dev/null && curl -fsSL --max-time 60 "$url" -o "$dest/antsy.tgz" 2>/dev/null; then
  tar -xzf "$dest/antsy.tgz" -C "$dest" && rm -f "$dest/antsy.tgz"
  echo "antsy installed from $url to $dest/antsy"
elif command -v cargo >/dev/null; then
  echo "no release binary for ${target:-this platform}; building with cargo (one small crate, four deps)"
  if [ -n "$root" ]; then
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo build --release --locked --manifest-path "$root/Cargo.toml"
    cp "$root/target/release/antsy" "$dest/antsy"
  else
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo install --git https://github.com/shinyobjectz/antsy --locked --root "${ANTSY_HOME:-$HOME/.antsy}"
  fi
  echo "antsy built and installed to $dest/antsy"
else
  echo "antsy: no release binary for this platform and no cargo to build one. Install Rust (https://rustup.rs) and rerun." >&2
  exit 1
fi
case ":$PATH:" in *":$dest:"*) ;; *) echo "add to PATH: export PATH=\"$dest:\$PATH\"" ;; esac
"$dest/antsy" doctor
