#!/bin/bash
# Pílula (fastwisper.pill): instala, habilita, idempotente sem restart do shell
# quando o QML não mudou, restart único quando mudou, rollback/uninstall
# removem. HOME/XDG/PATH descartáveis; nenhum shell, serviço ou daemon real.
set -euo pipefail
# Mensagens do instalador em inglês, qualquer que seja o locale do host.
unset LC_ALL; export LC_MESSAGES=C
repo=$(cd "$(dirname "$0")/../../.." && pwd)
# Lógica pura (onda adaptativa, NaN, meter, âmbar por contador) em node.
command -v node >/dev/null || { echo 'node ausente: necessário para testar PillLogic.js.' >&2; exit 1; }
node "$repo/packaging/omarchy/tests/pill-logic.mjs"
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
mkdir -p "$scratch/fixture" "$scratch/build" "$scratch/mock"
cp -r "$repo/packaging" "$scratch/fixture/"
repo=$scratch/fixture
export HOME="$scratch/home with spaces"
export XDG_CONFIG_HOME="$HOME/config" XDG_DATA_HOME="$HOME/data" XDG_STATE_HOME="$HOME/state"
export FASTWISPER_BUILD_DIR="$scratch/build" CALL_LOG="$scratch/calls"
mkdir -p "$XDG_CONFIG_HOME/fastwisper"
printf '[audio]\ndevice = "mic genérico"\n' > "$XDG_CONFIG_HOME/fastwisper/config.toml"
cat > "$FASTWISPER_BUILD_DIR/fastwisper" <<'MOCK'
#!/bin/bash
printf 'fastwisper %s\n' "$*" >> "$CALL_LOG"
[[ "$*" == 'ollama apply' || "$*" == 'hotkeys apply --no-reload' ]] || exit 99
MOCK
printf '#!/bin/bash\nexit 99\n' > "$FASTWISPER_BUILD_DIR/fastwisperd"
cat > "$scratch/mock/omarchy" <<'MOCK'
#!/bin/bash
printf 'omarchy %s\n' "$*" >> "$CALL_LOG"
shell_json="$XDG_CONFIG_HOME/omarchy/shell.json"
mkdir -p "$XDG_CONFIG_HOME/omarchy"
[[ -f "$shell_json" ]] || echo '{}' > "$shell_json"
case "$*" in
  'plugin enable fastwisper.pill') filter='.plugins = ((.plugins // []) + [{"id":"fastwisper.pill"}])' ;;
  *) exit 99 ;;
esac
jq "$filter" "$shell_json" > "$shell_json.tmp" && mv "$shell_json.tmp" "$shell_json"
MOCK
for command in systemctl pgrep hyprctl omarchy-shell omarchy-restart-shell; do
  printf '#!/bin/bash\nprintf "%s %%s\\n" "$*" >> "$CALL_LOG"\n' "$command" > "$scratch/mock/$command"
done
for command in curl wget kill pkill; do
  printf '#!/bin/bash\necho "forbidden %s" >> "$CALL_LOG"\nexit 99\n' "$command" > "$scratch/mock/$command"
done
chmod +x "$scratch/mock/"* "$FASTWISPER_BUILD_DIR/"*
export PATH="$scratch/mock:$PATH"
pill="$XDG_CONFIG_HOME/omarchy/plugins/fastwisper.pill"
install_pill() { : > "$CALL_LOG"; bash "$repo/packaging/omarchy/install.sh" --offline "$@" > /dev/null 2> "$scratch/stderr"; }
run() { bash "$repo/packaging/omarchy/$1.sh" > /dev/null; }
restarts() { grep -c '^omarchy-restart-shell' "$CALL_LOG" || true; }
# `! cmd` não dispara o `set -e`; a negação precisa sair explicitamente.
refute() { if "$@"; then echo "asserção negada falhou: $*" >&2; exit 1; fi; }

# Opt-out de ativação: copia a pílula sem chamar nada.
: > "$CALL_LOG"
bash "$repo/packaging/omarchy/install.sh" --no-activate > /dev/null 2>&1
[[ -f "$pill/manifest.json" && -f "$pill/Pill.qml" && -f "$pill/PillLogic.js" ]]
[[ ! -s "$CALL_LOG" ]]
run uninstall
[[ ! -e "$pill/manifest.json" && ! -e "$pill/Pill.qml" && ! -e "$pill/PillLogic.js" ]]

# Instalação nova: manifest panel/keepLoaded, CLI absoluta, habilita e só rescan.
install_pill
jq -e '.id == "fastwisper.pill" and .kinds == ["panel"] and .keepLoaded == true and .entryPoints.panel == "Pill.qml"' "$pill/manifest.json" > /dev/null
grep -qF "readonly property string cli: \"$HOME/.local/bin/fastwisper\"" "$pill/Pill.qml"
refute grep -q '@FASTWISPER_CLI@' "$pill/Pill.qml"
[[ $(stat -c '%a' "$pill/Pill.qml") == 644 ]]
cmp "$repo/packaging/omarchy/pill-plugin/PillLogic.js" "$pill/PillLogic.js"
[[ $(stat -c '%a' "$pill/PillLogic.js") == 644 ]]
grep -qxF 'import "PillLogic.js" as Logic' "$pill/Pill.qml"
grep -qxF 'omarchy plugin enable fastwisper.pill' "$CALL_LOG"
grep -qxF 'omarchy-shell shell rescanPlugins' "$CALL_LOG"
[[ $(restarts) == 0 ]]
jq -e 'any(.plugins[]; .id == "fastwisper.pill")' "$XDG_CONFIG_HOME/omarchy/shell.json" > /dev/null
# O widget da barra é um plugin separado: nada de fastwisper.dictation.
[[ ! -e "$XDG_CONFIG_HOME/omarchy/plugins/fastwisper.dictation" ]]
refute grep -q 'dictation\|plugin add' "$CALL_LOG"
cp "$pill/Pill.qml" "$scratch/pill-A"

# Repetição sem mudanças: mesmos bytes/mtimes/ledger, sem omarchy nem restart.
cp "$XDG_STATE_HOME/fastwisper/packaging/current" "$scratch/current"
find "$HOME" -type f -printf '%p %T@ %m\n' | sort > "$scratch/files-before"
install_pill
find "$HOME" -type f -printf '%p %T@ %m\n' | sort > "$scratch/files-after"
cmp "$scratch/files-before" "$scratch/files-after"
cmp "$scratch/current" "$XDG_STATE_HOME/fastwisper/packaging/current"
refute grep -q '^omarchy' "$CALL_LOG"

# Upgrade só do binário: sem restart.
echo '# nova versão' >> "$FASTWISPER_BUILD_DIR/fastwisper"
install_pill
[[ $(restarts) == 0 ]]

# QML da pílula mudou: restart uma vez; a repetição seguinte não reinicia.
echo '// upgrade da pílula' >> "$repo/packaging/omarchy/pill-plugin/Pill.qml"
install_pill
[[ $(restarts) == 1 ]]
grep -q 'upgrade da pílula' "$pill/Pill.qml"
install_pill
[[ $(restarts) == 0 ]]
# Só a lógica mudou: o shell também precisa reiniciar (uma vez).
echo '// lógica nova' >> "$repo/packaging/omarchy/pill-plugin/PillLogic.js"
install_pill
[[ $(restarts) == 1 ]]
grep -q 'lógica nova' "$pill/PillLogic.js"
install_pill
[[ $(restarts) == 0 ]]

# Rollback volta o QML anterior sem ativar nada; uninstall remove a pílula.
: > "$CALL_LOG"
run rollback
refute grep -q 'lógica nova' "$pill/PillLogic.js"
run rollback
[[ ! -s "$CALL_LOG" ]]
cmp "$scratch/pill-A" "$pill/Pill.qml"
run uninstall
[[ ! -e "$pill/manifest.json" && ! -e "$pill/Pill.qml" && ! -e "$pill/PillLogic.js" ]]

# Plugin removido do shell.json volta a ser habilitado (o opt-out é a config).
install_pill
jq '.plugins = []' "$XDG_CONFIG_HOME/omarchy/shell.json" > "$scratch/shell.json"
mv "$scratch/shell.json" "$XDG_CONFIG_HOME/omarchy/shell.json"
install_pill
grep -qxF 'omarchy plugin enable fastwisper.pill' "$CALL_LOG"

# Edição externa da pílula prevalece no upgrade e no uninstall.
echo '// escolha externa' >> "$pill/Pill.qml"
echo '// segundo upgrade' >> "$repo/packaging/omarchy/pill-plugin/Pill.qml"
install_pill
grep -q 'Preserved (later choice).*Pill.qml' "$scratch/stderr"
run uninstall
grep -q 'escolha externa' "$pill/Pill.qml"
[[ ! -e "$pill/manifest.json" ]]
refute grep -q forbidden "$CALL_LOG"
echo 'pílula: PASS (instala, habilita, idempotente sem restart, restart só com QML novo, rollback/uninstall removem, edição externa preservada)'
