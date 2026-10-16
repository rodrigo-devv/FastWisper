#!/usr/bin/env bash
# Versão e nome do tarball, lidos do Cargo.toml do workspace (fonte única).
release_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
release_version() {
  local version
  version=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\([^"]*\)"$/\1/p' "$release_root/Cargo.toml")
  [[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || { echo "versão inválida no Cargo.toml: '$version'" >&2; return 1; }
  printf '%s\n' "$version"
}
release_name() { printf 'fastwisper-%s-x86_64-linux\n' "$1"; }
# Arquivos do payload: o que o git conhece (rastreado ou novo não ignorado),
# sem os testes do pacote. Nunca lixo local nem target/.
release_payload() {
  git -C "$release_root" ls-files --cached --others --exclude-standard -- packaging config/config.example.toml \
    | grep -v '^packaging/omarchy/tests/' | sort -u
}
