#!/usr/bin/env bash
# Contrato do tarball sem compilar: binários falsos, HOME/XDG temporários.
set -euo pipefail
source "$(dirname -- "$0")/../common.sh"
root=$release_root
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
version=$(release_version)
name=$(release_name "$version")
fake() { # fake DIR NOME VERSÃO
  mkdir -p "$1"
  printf '#!/bin/sh\n[ "$1" = --version ] && echo "%s %s"\nexit 0\n' "$2" "$3" > "$1/$2"
  chmod +x "$1/$2"
}
for binary in fastwisper fastwisperd; do fake "$scratch/build" "$binary" "$version"; done
fake "$scratch/hub" fastwisper-hub "$version"
export SOURCE_DATE_EPOCH=1700000000
bash "$root/scripts/release/package.sh" "$scratch/build" "$scratch/hub" "$scratch/dist" > /dev/null
bash "$root/scripts/release/verify.sh" "$scratch/dist" > /dev/null
cmp -s "$root/docs/release/v$version.md" "$scratch/dist/release-notes.md"
# Reprodutível: a mesma entrada gera o mesmo SHA-256.
bash "$root/scripts/release/package.sh" "$scratch/build" "$scratch/hub" "$scratch/again" > /dev/null
cmp -s "$scratch/dist/SHA256SUMS" "$scratch/again/SHA256SUMS"
# Checksum adulterado, versão divergente ou hub ausente: falha alto.
cp -r "$scratch/dist" "$scratch/tampered"
printf 'x' >> "$scratch/tampered/$name.tar.gz"
if bash "$root/scripts/release/verify.sh" "$scratch/tampered" > /dev/null 2>&1; then echo 'checksum adulterado aceito' >&2; exit 1; fi
fake "$scratch/old" fastwisper 0.0.1
cp "$scratch/build/fastwisperd" "$scratch/old/"
if bash "$root/scripts/release/package.sh" "$scratch/old" "$scratch/hub" "$scratch/bad" > /dev/null 2>&1; then echo 'versão divergente aceita' >&2; exit 1; fi
if bash "$root/scripts/release/package.sh" "$scratch/build" "$scratch/missing" "$scratch/bad" > /dev/null 2>&1; then echo 'hub ausente aceito' >&2; exit 1; fi
[[ ! -e $scratch/bad ]]
# Opções do instalador passam pelo wrapper: --no-hub instala sem o hub.
mkdir "$scratch/x"
tar -xzf "$scratch/dist/$name.tar.gz" -C "$scratch/x"
env HOME="$scratch/nohub" XDG_CONFIG_HOME="$scratch/nohub/config" XDG_DATA_HOME="$scratch/nohub/data" \
  XDG_STATE_HOME="$scratch/nohub/state" XDG_RUNTIME_DIR="$scratch/nohub/runtime" \
  "$scratch/x/$name/install.sh" --no-activate --no-hub > /dev/null 2>&1
[[ -x $scratch/nohub/.local/bin/fastwisperd && ! -e $scratch/nohub/.local/bin/fastwisper-hub ]]
echo 'release contract: ok'
