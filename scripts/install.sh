#!/usr/bin/env sh
# Install the aieo binary.
#
#   curl -fsSL https://raw.githubusercontent.com/web-casa/aieo-cli/main/scripts/install.sh | sh
#
# AIEO_VERSION   a release (1.0.0), "latest" (default), or "source" to build
#                from a checkout with cargo (the GitHub Action's default)
# AIEO_INSTALL_DIR  where the binary goes (default ~/.local/bin; in GitHub
#                Actions a runner temp directory added to PATH)
# AIEO_REPOSITORY   owner/repo of the releases (default web-casa/aieo-cli)
set -eu

version="${AIEO_VERSION:-latest}"
repo="${AIEO_REPOSITORY:-web-casa/aieo-cli}"
if [ -n "${GITHUB_PATH:-}" ]; then
  bin_dir="${AIEO_INSTALL_DIR:-${RUNNER_TEMP:-/tmp}/aieo-bin}"
else
  bin_dir="${AIEO_INSTALL_DIR:-$HOME/.local/bin}"
fi
mkdir -p "$bin_dir"

if [ "$version" = "source" ]; then
  root="${ACTION_PATH:-$(cd "$(dirname "$0")/.." && pwd)}"
  cargo build --release --locked --manifest-path "$root/Cargo.toml" -p aieo-cli
  if [ -f "$root/target/release/aieo.exe" ]; then
    cp "$root/target/release/aieo.exe" "$bin_dir/"
  else
    cp "$root/target/release/aieo" "$bin_dir/"
  fi
else
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) target=x86_64-unknown-linux-musl ;;
    Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-musl ;;
    Darwin-arm64) target=aarch64-apple-darwin ;;
    Darwin-x86_64) target=x86_64-apple-darwin ;;
    MINGW*-x86_64 | MSYS*-x86_64 | CYGWIN*-x86_64) target=x86_64-pc-windows-msvc ;;
    *) echo "aieo: no prebuilt binary for $(uname -s)-$(uname -m); use: cargo install --git https://github.com/$repo aieo-cli" >&2; exit 1 ;;
  esac
  if [ "$version" = "latest" ]; then
    # the redirect of /releases/latest names the tag
    tag=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest" | sed 's#.*/tag/##')
    [ -n "$tag" ] || { echo "aieo: could not find the latest release of $repo" >&2; exit 1; }
    version="${tag#v}"
  fi
  version="${version#v}"
  case "$target" in
    *windows*) ext=zip exe=aieo.exe ;;
    *) ext=tar.gz exe=aieo ;;
  esac
  asset="aieo-v${version}-${target}.${ext}"
  url="https://github.com/${repo}/releases/download/v${version}/${asset}"
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  curl -fsSL "$url" -o "$tmp/$asset"
  curl -fsSL "$url.sha256" -o "$tmp/$asset.sha256"
  (cd "$tmp" && if command -v sha256sum >/dev/null; then sha256sum -c "$asset.sha256"; else shasum -a 256 -c "$asset.sha256"; fi) >/dev/null
  if [ "$ext" = zip ]; then
    unzip -q -o "$tmp/$asset" -d "$tmp"
  else
    tar -xzf "$tmp/$asset" -C "$tmp"
  fi
  mv "$tmp/$exe" "$bin_dir/$exe"
fi

[ -f "$bin_dir/aieo.exe" ] || chmod +x "$bin_dir/aieo"
if [ -n "${GITHUB_PATH:-}" ]; then
  echo "$bin_dir" >> "$GITHUB_PATH"
fi
"$bin_dir/aieo" --version
case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) [ -n "${GITHUB_PATH:-}" ] || echo "add $bin_dir to PATH to run aieo" ;;
esac
