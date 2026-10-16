#!/usr/bin/env bash
# Monta o tarball da release a partir de binários já compilados; não compila.
# Uso: scripts/release/package.sh BUILD_DIR HUB_BUILD_DIR OUT_DIR
# Saída em OUT_DIR: fastwisper-<versão>-x86_64-linux.tar.gz, SHA256SUMS e
# release-notes.md (corpo da release, fora do SHA256SUMS).
set -euo pipefail
source "$(dirname -- "$0")/common.sh"
[[ $# == 3 ]] || { echo 'Uso: package.sh BUILD_DIR HUB_BUILD_DIR OUT_DIR' >&2; exit 2; }
build_dir=$1 hub_dir=$2 out=$3
version=$(release_version)
name=$(release_name "$version")
notes=$release_root/docs/release/v$version.md
[[ -f $notes ]] || { echo "faltam as notas da release: docs/release/v$version.md" >&2; exit 1; }
for binary in "$build_dir/fastwisper" "$build_dir/fastwisperd" "$hub_dir/fastwisper-hub"; do
  [[ -f $binary && -x $binary ]] || { echo "binário ausente: $binary" >&2; exit 1; }
done
for binary in fastwisper fastwisperd; do
  reported=$("$build_dir/$binary" --version)
  [[ $reported == "$binary $version" ]] || { echo "$binary --version: '$reported'; esperado '$binary $version'" >&2; exit 1; }
done
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
dir=$stage/$name
mkdir -p "$dir/bin"
install -m 0755 "$build_dir/fastwisper" "$build_dir/fastwisperd" "$hub_dir/fastwisper-hub" "$dir/bin/"
while IFS= read -r file; do
  mkdir -p "$dir/$(dirname -- "$file")"
  if [[ -x $release_root/$file ]]; then mode=0755; else mode=0644; fi
  install -m "$mode" "$release_root/$file" "$dir/$file"
done < <(release_payload)
[[ -x $dir/packaging/omarchy/install.sh ]] || { echo 'payload sem packaging/omarchy/install.sh' >&2; exit 1; }
install -m 0755 "$release_root/scripts/release/install.sh" "$dir/install.sh"
install -m 0644 "$release_root/README.md" "$release_root/README.pt-BR.md" "$release_root/CHANGELOG.md" "$release_root/LICENSE" "$dir/"
find "$dir" -type d -exec chmod 0755 {} +
# Reprodutível: mesma árvore e mesmo SOURCE_DATE_EPOCH, mesmo SHA-256.
epoch=${SOURCE_DATE_EPOCH:-$(git -C "$release_root" log -1 --format=%ct 2>/dev/null || date +%s)}
mkdir -p "$out"
out=$(cd "$out" && pwd)
tar --sort=name --format=gnu --owner=0 --group=0 --numeric-owner --mtime="@$epoch" \
  -C "$stage" -cf - "$name" | gzip -9n > "$out/$name.tar.gz"
(cd "$out" && sha256sum "$name.tar.gz" > SHA256SUMS)
install -m 0644 "$notes" "$out/release-notes.md"
echo "$out/$name.tar.gz"
