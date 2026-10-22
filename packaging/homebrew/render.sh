#!/usr/bin/env sh
# Fill aieo.rb.tmpl from the release assets' .sha256 files:
#   packaging/homebrew/render.sh <version> <assets dir> [owner/repo] > aieo.rb
set -eu
version="${1:?version}"; assets="${2:?assets dir}"; repo="${3:-web-casa/aieo-cli}"
here=$(cd "$(dirname "$0")" && pwd)
out=$(sed -e "s#@VERSION@#${version}#g" -e "s#@REPO@#${repo}#g" "$here/aieo.rb.tmpl")
for target in aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-musl x86_64-unknown-linux-musl; do
  file="$assets/aieo-v${version}-${target}.tar.gz.sha256"
  [ -f "$file" ] || { echo "missing $file" >&2; exit 1; }
  sum=$(cut -d' ' -f1 "$file")
  out=$(printf '%s' "$out" | sed "s#@${target}@#${sum}#")
done
printf '%s\n' "$out"
