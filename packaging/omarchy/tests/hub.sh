#!/bin/bash
# Nenhum build, serviço ou desktop real: dependências e binários isolados.
set -euo pipefail
# Mensagens do instalador em inglês, qualquer que seja o locale do host.
unset LC_ALL; export LC_MESSAGES=C
# `! cmd` não dispara o `set -e`; a negação precisa sair explicitamente.
refute() { if "$@"; then echo "asserção negada falhou: $*" >&2; exit 1; fi; }
repo=$(cd "$(dirname "$0")/../../.." && pwd)
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
mkdir -p "$scratch/repo/packaging" "$scratch/mock" "$scratch/repo/target/release" "$scratch/hub"
cp -r "$repo/packaging/omarchy" "$scratch/repo/packaging/"
cp -r "$repo/packaging/systemd" "$scratch/repo/packaging/"
cp "$repo/README.md" "$scratch/repo/"
for binary in fastwisper fastwisperd; do
  printf '#!/bin/bash\nexit 0\n' > "$scratch/repo/target/release/$binary"
  chmod +x "$scratch/repo/target/release/$binary"
done
printf '#!/bin/bash\nexit 0\n' > "$scratch/hub/fastwisper-hub"
chmod +x "$scratch/hub/fastwisper-hub"
# PATH fechado: utilitários de packaging, sem node/npm por padrão.
for utility in tac rmdir env bash dirname mkdir chmod flock mktemp grep sha256sum cut cp cat stat wc install mv rm sed cmp; do
  ln -s "$(command -v "$utility")" "$scratch/mock/$utility"
done
cat > "$scratch/mock/cargo" <<'MOCK'
#!/bin/bash
printf 'cargo\n' >> "$HUB_CALL_LOG"
MOCK
chmod +x "$scratch/mock/cargo"
run() {
  local name=$1; shift
  env HOME="$scratch/$name" XDG_CONFIG_HOME="$scratch/$name/config" XDG_DATA_HOME="$scratch/$name/data" XDG_STATE_HOME="$scratch/$name/state" PATH="$scratch/mock" HUB_CALL_LOG="$scratch/calls" "$@"
}
# --no-hub não consulta dependências nem tenta npm.
run optout bash "$scratch/repo/packaging/omarchy/install.sh" --no-activate --no-hub > "$scratch/optout.log" 2>&1
[[ -x "$scratch/optout/.local/bin/fastwisper" ]]
[[ ! -e "$scratch/optout/.local/bin/fastwisper-hub" ]]
[[ ! -e "$scratch/optout/data/doc/fastwisper/README.md" ]]
refute grep -q 'continuing without the hub' "$scratch/optout.log"
# O motivo fica gravado para `fastwisper hub` explicar a ausência.
status() { cat "$scratch/$1/state/fastwisper/packaging/hub-status"; }
[[ $(status optout) == no_hub ]]
# Sem node/npm: avisa e mantém instalação do ditado funcional.
run missingnode bash "$scratch/repo/packaging/omarchy/install.sh" --no-activate > "$scratch/node.log" 2>&1
grep -q 'node/npm not found; continuing without the hub' "$scratch/node.log"
[[ -x "$scratch/missingnode/.local/bin/fastwisperd" ]]
[[ ! -e "$scratch/missingnode/data/applications/fastwisper.desktop" ]]
[[ $(status missingnode) == missing_node ]]
# Node presente, WebKit ausente: mesmo contrato.
for utility in node npm; do
  printf '#!/bin/bash\necho unexpected >> "$HUB_CALL_LOG"\nexit 99\n' > "$scratch/mock/$utility"
  chmod +x "$scratch/mock/$utility"
done
printf '#!/bin/bash\nexit 1\n' > "$scratch/mock/pkg-config"
chmod +x "$scratch/mock/pkg-config"
run missingwebkit bash "$scratch/repo/packaging/omarchy/install.sh" --no-activate > "$scratch/webkit.log" 2>&1
grep -q 'webkit2gtk-4.1 not found; continuing without the hub' "$scratch/webkit.log"
[[ -x "$scratch/missingwebkit/.local/bin/fastwisper" ]]
refute grep -q unexpected "$scratch/calls"
[[ $(status missingwebkit) == missing_webkit ]]
# Candidatos externos: hub + desktop entram no ledger e saem no rollback.
run 'with spaces' env FASTWISPER_HUB_BUILD_DIR="$scratch/hub" bash "$scratch/repo/packaging/omarchy/install.sh" --no-activate > "$scratch/hub.log" 2>&1
[[ -x "$scratch/with spaces/.local/bin/fastwisper-hub" ]]
[[ $(status 'with spaces') == installed ]]
grep -qF "Exec=\"$scratch/with spaces/.local/bin/fastwisper\" hub" "$scratch/with spaces/data/applications/fastwisper.desktop"
# Agrupamento no lançador: o app_id da janela do hub (GTK) é o nome do
# executável, o pacote do Tauri; conferido com `hyprctl clients -j`.
hub_package=$(sed -n 's/^name = "\(.*\)"$/\1/p' "$repo/ui/src-tauri/Cargo.toml" | head -n 1)
[[ "$hub_package" == fastwisper-hub ]]
grep -qxF "StartupWMClass=$hub_package" "$scratch/with spaces/data/applications/fastwisper.desktop"
[[ $(grep -c '^StartupWMClass=' "$scratch/with spaces/data/applications/fastwisper.desktop") == 1 ]]
# Guia local do item Ajuda, só com o hub.
cmp -s "$repo/README.md" "$scratch/with spaces/data/doc/fastwisper/README.md"
run 'with spaces' bash "$scratch/repo/packaging/omarchy/rollback.sh" > /dev/null
[[ ! -e "$scratch/with spaces/.local/bin/fastwisper-hub" ]]
[[ ! -e "$scratch/with spaces/data/doc/fastwisper/README.md" ]]
[[ ! -e "$scratch/with spaces/data/applications/fastwisper.desktop" ]]
# Edição externa do desktop prevalece no uninstall.
run edited env FASTWISPER_HUB_BUILD_DIR="$scratch/hub" bash "$scratch/repo/packaging/omarchy/install.sh" --no-activate > /dev/null
printf '\n# edição externa\n' >> "$scratch/edited/data/applications/fastwisper.desktop"
run edited bash "$scratch/repo/packaging/omarchy/uninstall.sh" > /dev/null
grep -q 'edição externa' "$scratch/edited/data/applications/fastwisper.desktop"
# --offline mantém npm/cargo sem downloads no build padrão do hub.
mkdir -p "$scratch/repo/ui/src-tauri/target/release"
cp "$scratch/hub/fastwisper-hub" "$scratch/repo/ui/src-tauri/target/release/fastwisper-hub"
printf '#!/bin/bash\nexit 0\n' > "$scratch/mock/pkg-config"
cat > "$scratch/mock/npm" <<'MOCK'
#!/bin/bash
printf '%s\n' "$*" >> "$HUB_CALL_LOG"
MOCK
run offline bash "$scratch/repo/packaging/omarchy/install.sh" --no-activate --offline > /dev/null
grep -qx 'ci --offline' "$scratch/calls"
grep -qx 'run tauri -- build --no-bundle -- --locked --offline' "$scratch/calls"
[[ -x "$scratch/offline/.local/bin/fastwisper-hub" ]]
# Build do hub falhando (ex.: --offline sem cache do npm) não bloqueia o ditado.
printf '#!/bin/bash\nexit 1\n' > "$scratch/mock/npm"
run buildfail bash "$scratch/repo/packaging/omarchy/install.sh" --no-activate --offline > "$scratch/buildfail.log" 2>&1
grep -q 'Hub build failed; continuing without the hub' "$scratch/buildfail.log"
[[ -x "$scratch/buildfail/.local/bin/fastwisperd" ]]
[[ ! -e "$scratch/buildfail/.local/bin/fastwisper-hub" ]]
[[ ! -e "$scratch/buildfail/data/applications/fastwisper.desktop" ]]
[[ $(status buildfail) == build_failed ]]
# Build externo sem o hub: motivo próprio.
run external env FASTWISPER_BUILD_DIR="$scratch/repo/target/release" bash "$scratch/repo/packaging/omarchy/install.sh" --no-activate > /dev/null 2>&1
[[ $(status external) == external_build ]]
echo 'hub packaging: ok' 
