#!/bin/bash
# Caminho de instalação estranho (A-07): HOME com aspas, barra invertida,
# `|`, `&`, `$` e espaços. Os templates Lua/QML recebem o caminho escapado
# (nunca por sed), o bloco do bindings.lua carrega o arquivo certo e o comando
# do atalho chega à CLI falsa. Um Alt+Space do usuário não impede o bloco nem
# é alterado. Instalação --no-activate em HOME/XDG descartáveis; nada real.
set -euo pipefail
# Mensagens do instalador em inglês, qualquer que seja o locale do host.
unset LC_ALL; export LC_MESSAGES=C
refute() { if "$@"; then echo "asserção negada falhou: $*" >&2; exit 1; fi; }
repo=$(cd "$(dirname "$0")/../../.." && pwd)
lua=''
for candidate in lua5.5 lua5.4 lua; do
  if command -v "$candidate" >/dev/null; then lua=$candidate; break; fi
done
[[ -n "$lua" ]] || { echo 'Lua ausente (lua5.4 ou mais nova): o Hyprland falso precisa dele.' >&2; exit 1; }
command -v python3 >/dev/null || { echo 'python3 ausente: necessário para decodificar as strings QML.' >&2; exit 1; }
harness=$repo/packaging/omarchy/tests/fake-hyprland.lua
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
mkdir -p "$scratch/fixture" "$scratch/build" "$scratch/mock"
cp -r "$repo/packaging" "$scratch/fixture/"
export HOME="$scratch/home \"aspas\" barra\\invertida |pipe &e 'apóstrofo' \$HOME"
export XDG_CONFIG_HOME="$HOME/config" XDG_DATA_HOME="$HOME/data" XDG_STATE_HOME="$HOME/state"
export FASTWISPER_BUILD_DIR="$scratch/build" CALL_LOG="$scratch/calls"
mkdir -p "$XDG_CONFIG_HOME/hypr" "$XDG_CONFIG_HOME/fastwisper"
printf '[audio]\ndevice = "mic genérico"\n' > "$XDG_CONFIG_HOME/fastwisper/config.toml"
# Binds do usuário: um Alt+Space dele (estilo Omarchy) e um comentário.
cat > "$XDG_CONFIG_HOME/hypr/bindings.lua" <<'LUA'
-- o.bind("SUPER + ALT + V", "comentado não conta", "x")
o.bind("ALT + SPACE", "Meu lançador", "meu-lancador")
LUA
cp "$XDG_CONFIG_HOME/hypr/bindings.lua" "$scratch/bindings-user"
cat > "$FASTWISPER_BUILD_DIR/fastwisper" <<'MOCK'
#!/bin/bash
printf 'fastwisper %s\n' "$*" >> "$CALL_LOG"
MOCK
printf '#!/bin/bash\nexit 99\n' > "$FASTWISPER_BUILD_DIR/fastwisperd"
for command in systemctl hyprctl omarchy omarchy-shell omarchy-restart-shell wtype curl wget; do
  printf '#!/bin/bash\necho "forbidden %s" >> "$CALL_LOG"\nexit 99\n' "$command" > "$scratch/mock/$command"
done
chmod +x "$scratch/mock/"* "$FASTWISPER_BUILD_DIR/"*
export PATH="$scratch/mock:$PATH"

bash "$scratch/fixture/packaging/omarchy/install.sh" --no-activate --no-hub > /dev/null 2> "$scratch/stderr"
installed=$XDG_DATA_HOME/fastwisper/hyprland.lua
[[ -f "$installed" && -x "$HOME/.local/bin/fastwisper" ]]
refute grep -q '@FASTWISPER_' "$installed"

# Conflito com o Alt+Space do usuário: avisa só o atalho de ditar (o
# comentado não conta), adiciona o bloco uma vez e não toca no resto.
grep -qF 'ALT+SPACE' "$scratch/stderr"
grep -qF 'hotkey.dictate' "$scratch/stderr"
refute grep -qF 'hotkey.paste_last' "$scratch/stderr"
bindings=$XDG_CONFIG_HOME/hypr/bindings.lua
[[ $(grep -cxF -- '-- BEGIN fastwisper' "$bindings") == 1 ]]
[[ $(head -n 2 "$bindings") == $(cat "$scratch/bindings-user") ]]

# O bloco do bindings.lua, executado em Lua, carrega exatamente o arquivo instalado.
loaded=$(EXPECTED="$installed" BINDINGS="$bindings" "$lua" -e '
  o = { bind = function() end }
  hl = {}
  local seen
  dofile = function(path) seen = path end
  assert(loadfile(os.getenv("BINDINGS")))()
  assert(seen == os.getenv("EXPECTED"), "dofile recebeu " .. tostring(seen))
  print("ok")')
[[ "$loaded" == ok ]]

# O Lua instalado lê o arquivo de atalhos no caminho estranho.
printf 'version 1\ndictate CTRL+ALT+F9 75 37 105 64 108\nhands_free ALT+SHIFT+SPACE\npaste_last SUPER+ALT+V\n' \
  > "$XDG_DATA_HOME/fastwisper/hotkeys.conf"
binds=$("$lua" "$harness" "$installed" binds)
grep -qxF 'bind CTRL + ALT + F9 = FastWisper: dictate (hold)' <<< "$binds"

# O comando montado pelo Lua é uma palavra de shell válida: chega à CLI.
command=$("$lua" "$harness" "$installed" press 'CTRL + ALT + F9')
[[ "$command" == "exec "* ]]
: > "$CALL_LOG"
bash -c "${command#exec }"
[[ $(cat "$CALL_LOG") == 'fastwisper ctl press' ]]

# QML da pílula: a string decodificada é o caminho da CLI.
qml_cli() {
  local line
  line=$(grep -m1 'readonly property string cli:' "$1")
  python3 -I -c 'import json, sys; print(json.loads(sys.argv[1].split("cli: ", 1)[1]))' "$line"
}
[[ $(qml_cli "$XDG_CONFIG_HOME/omarchy/plugins/fastwisper.pill/Pill.qml") == "$HOME/.local/bin/fastwisper" ]]

# Reinstalar não duplica; desinstalar devolve o bindings.lua do usuário.
bash "$scratch/fixture/packaging/omarchy/install.sh" --no-activate --no-hub > /dev/null 2>&1
[[ $(grep -cxF -- '-- BEGIN fastwisper' "$bindings") == 1 ]]
bash "$scratch/fixture/packaging/omarchy/uninstall.sh" > /dev/null
cmp "$scratch/bindings-user" "$bindings"
[[ ! -e "$installed" ]]
refute grep -q forbidden "$CALL_LOG"
echo 'caminhos: PASS (aspas, barra invertida, |, &, $ e espaços no Lua/QML; bloco com Alt+Space do usuário, sem duplicar)'
