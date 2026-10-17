#!/usr/bin/env bash
# Confere o que package.sh gerou (ou o artifact baixado da CI): SHA256SUMS,
# lista exata de arquivos, dono/modos, versão, bibliotecas e uma instalação
# --no-activate do tarball em HOME/XDG temporários, sem compilar nada.
# Uso: scripts/release/verify.sh DIST_DIR
set -euo pipefail
source "$(dirname -- "$0")/common.sh"
[[ $# == 1 ]] || { echo 'Uso: verify.sh DIST_DIR' >&2; exit 2; }
fail() { echo "verify: $*" >&2; exit 1; }
dist=$(cd "$1" && pwd)
version=$(release_version)
name=$(release_name "$version")
tarball=$name.tar.gz
[[ -f $dist/$tarball ]] || fail "falta $tarball"
[[ $(cat "$dist/SHA256SUMS") =~ ^[0-9a-f]{64}\ \ $tarball$ ]] || fail 'SHA256SUMS deve ter só a linha do tarball'
(cd "$dist" && sha256sum --strict --quiet -c SHA256SUMS) || fail 'checksum divergente'
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
# Lista exata: tudo sob $name/, sem testes, target/ nem arquivos fora do payload.
{
  printf '%s\n' bin/fastwisper bin/fastwisperd bin/fastwisper-hub install.sh \
    README.md README.pt-BR.md CHANGELOG.md LICENSE
  release_payload
} | sed "s|^|$name/|" | sort > "$scratch/expected"
tar -tzf "$dist/$tarball" | grep -v '/$' | sort > "$scratch/actual"
diff -u "$scratch/expected" "$scratch/actual" || fail 'lista de arquivos do tarball divergente'
owners=$(tar --numeric-owner -tvzf "$dist/$tarball" | awk '{print $2}' | sort -u)
[[ $owners == 0/0 ]] || fail "donos inesperados: $owners"
mkdir "$scratch/x"
tar -xzf "$dist/$tarball" -C "$scratch/x"
pkg=$scratch/x/$name
for binary in fastwisper fastwisperd fastwisper-hub install.sh packaging/omarchy/install.sh packaging/omarchy/uninstall.sh packaging/omarchy/rollback.sh; do
  [[ -x $pkg/bin/$binary || -x $pkg/$binary ]] || fail "$binary sem permissão de execução"
done
for binary in fastwisper fastwisperd; do
  [[ $("$pkg/bin/$binary" --version) == "$binary $version" ]] || fail "$binary --version divergente"
done
# Binários ELF: nenhuma biblioteca dinâmica faltando neste sistema.
for binary in fastwisper fastwisperd fastwisper-hub; do
  if ldd "$pkg/bin/$binary" > "$scratch/ldd" 2>&1; then
    ! grep -q 'not found' "$scratch/ldd" || fail "$binary: $(grep 'not found' "$scratch/ldd")"
  fi
done
# Instalação isolada: nada de cargo/npm/node (o tarball não compila).
mkdir -p "$scratch/mock" "$scratch/home" "$scratch/runtime"
chmod 0700 "$scratch/runtime"
for tool in cargo npm node; do
  printf '#!/bin/sh\necho "%s chamado" >> "%s/calls"\nexit 99\n' "$tool" "$scratch" > "$scratch/mock/$tool"
  chmod +x "$scratch/mock/$tool"
done
isolated() {
  env -u HYPRLAND_INSTANCE_SIGNATURE -u FASTWISPER_BUILD_DIR -u FASTWISPER_HUB_BUILD_DIR \
    HOME="$scratch/home" XDG_CONFIG_HOME="$scratch/home/config" XDG_DATA_HOME="$scratch/home/data" \
    XDG_STATE_HOME="$scratch/home/state" XDG_RUNTIME_DIR="$scratch/runtime" PATH="$scratch/mock:$PATH" "$@"
}
isolated "$pkg/install.sh" --no-activate > "$scratch/install.log" 2>&1 || { cat "$scratch/install.log" >&2; fail 'install.sh --no-activate falhou'; }
[[ ! -e $scratch/calls ]] || fail "o instalador tentou compilar: $(cat "$scratch/calls")"
for binary in fastwisper fastwisperd fastwisper-hub; do
  cmp -s "$pkg/bin/$binary" "$scratch/home/.local/bin/$binary" || fail "$binary não instalado do tarball"
done
for file in data/applications/fastwisper.desktop data/doc/fastwisper/README.md data/fastwisper/hyprland.lua \
  config/systemd/user/fastwisperd.service config/omarchy/plugins/fastwisper.pill/Pill.qml; do
  [[ -f $scratch/home/$file ]] || fail "não instalado: $file"
done
# O widget da barra é um plugin separado do Omarchy: fora do tarball e da instalação.
! grep -q '/packaging/omarchy/shell-plugin/' "$scratch/actual" || fail 'tarball com o widget legado da barra'
[[ ! -e $scratch/home/config/omarchy/plugins/fastwisper.dictation ]] || fail 'instalou o widget legado da barra'
[[ $(isolated "$scratch/home/.local/bin/fastwisper" --version) == "fastwisper $version" ]] || fail 'fastwisper instalado não responde'
isolated bash "$pkg/packaging/omarchy/uninstall.sh" > "$scratch/uninstall.log" 2>&1 || { cat "$scratch/uninstall.log" >&2; fail 'uninstall.sh falhou'; }
for binary in fastwisper fastwisperd fastwisper-hub; do
  [[ ! -e $scratch/home/.local/bin/$binary ]] || fail "uninstall deixou $binary"
done
echo "verify: $tarball ok ($(cut -c1-12 "$dist/SHA256SUMS")…; $(wc -l < "$scratch/actual") arquivos; instalação --no-activate e uninstall em HOME temporário)"
