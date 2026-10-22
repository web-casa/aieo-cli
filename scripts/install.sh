#!/usr/bin/env bash
# Install the aieo binary for the GitHub Action: a release asset, or a build
# from the checked-out action source (AIEO_VERSION=source).
set -euo pipefail

version="${AIEO_VERSION:-source}"
bin_dir="${RUNNER_TEMP:-/tmp}/aieo-bin"
mkdir -p "$bin_dir"

if [ "$version" = "source" ]; then
  root="${ACTION_PATH:-$(cd "$(dirname "$0")/.." && pwd)}"
  cargo build --release --locked --manifest-path "$root/Cargo.toml" -p aieo-cli
  cp "$root/target/release/aieo" "$bin_dir/"
else
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) target=x86_64-unknown-linux-gnu ;;
    Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-gnu ;;
    Darwin-arm64) target=aarch64-apple-darwin ;;
    Darwin-x86_64) target=x86_64-apple-darwin ;;
    *) echo "unsupported runner: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
  esac
  repo="${AIEO_REPOSITORY:-web-casa/aieo-cli}"
  asset="aieo-v${version#v}-${target}.tar.gz"
  url="https://github.com/${repo}/releases/download/v${version#v}/${asset}"
  curl -fsSL "$url" -o "$bin_dir/$asset"
  curl -fsSL "$url.sha256" -o "$bin_dir/$asset.sha256"
  (cd "$bin_dir" && sha256sum -c "$asset.sha256")
  tar -xzf "$bin_dir/$asset" -C "$bin_dir"
fi

chmod +x "$bin_dir/aieo"
echo "$bin_dir" >> "${GITHUB_PATH:-/dev/null}"
"$bin_dir/aieo" --version
