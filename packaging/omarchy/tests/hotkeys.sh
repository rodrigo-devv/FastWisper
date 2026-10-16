#!/bin/bash
# Atalhos editáveis de ponta a ponta, sem tocar no desktop: instala com
# --no-activate em HOME/XDG descartáveis, muda os atalhos pela CLI real
# (`config set`, `config reload`, `hotkeys`) contra um `hyprctl` falso e roda o
# hyprland.lua instalado num Hyprland falso em Lua (fake-hyprland.lua):
# binds, soltura pela tecla e por qualquer modificador (esquerdo ou direito),
# recarga com gravação segurada (cancela, nunca cola), arquivo ausente,
# corrompido ou com tentativa de injeção (cai nos padrões) e conflitos.
# FASTWISPER_BIN: a CLI compilada (padrão: target/debug/fastwisper).
set -euo pipefail
refute() { if "$@"; then echo "asserção negada falhou: $*" >&2; exit 1; fi; }
repo=$(cd "$(dirname "$0")/../../.." && pwd)
cli_bin=${FASTWISPER_BIN:-$repo/target/debug/fastwisper}
[[ -x "$cli_bin" ]] || { echo "CLI ausente em $cli_bin; compile (cargo build -p fastwisper) ou defina FASTWISPER_BIN." >&2; exit 1; }
lua=''
for candidate in lua5.5 lua5.4 lua; do
  if command -v "$candidate" >/dev/null; then lua=$candidate; break; fi
done
[[ -n "$lua" ]] || { echo 'Lua ausente (lua5.4 ou mais nova): o Hyprland falso precisa dele.' >&2; exit 1; }
harness=$repo/packaging/omarchy/tests/fake-hyprland.lua
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
mkdir -p "$scratch/fixture" "$scratch/build" "$scratch/mock" "$scratch/runtime"
chmod 0700 "$scratch/runtime"
cp -r "$repo/packaging" "$scratch/fixture/"
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/config" XDG_DATA_HOME="$HOME/data" XDG_STATE_HOME="$HOME/state"
export XDG_RUNTIME_DIR="$scratch/runtime" FASTWISPER_BUILD_DIR="$scratch/build"
export HYPRLAND_INSTANCE_SIGNATURE=fake-e2 CALL_LOG="$scratch/calls" FAKE_BINDS="$scratch/binds.json"
mkdir -p "$XDG_CONFIG_HOME/hypr" "$XDG_CONFIG_HOME/fastwisper"
printf '%s\n' '-- binds do usuário' > "$XDG_CONFIG_HOME/hypr/bindings.lua"
printf '[audio]\ndevice = "mic genérico"\n' > "$XDG_CONFIG_HOME/fastwisper/config.toml"
cp "$cli_bin" "$FASTWISPER_BUILD_DIR/fastwisper"
printf '#!/bin/bash\nexit 99\n' > "$FASTWISPER_BUILD_DIR/fastwisperd"
chmod +x "$FASTWISPER_BUILD_DIR/"*
# hyprctl falso: binds -j lê o fixture (só leitura); reload só registra.
cat > "$scratch/mock/hyprctl" <<'MOCK'
#!/bin/bash
printf 'hyprctl %s\n' "$*" >> "$CALL_LOG"
case "$*" in
  'binds -j') cat "$FAKE_BINDS" ;;
  reload) echo ok ;;
  *) exit 99 ;;
esac
MOCK
for command in systemctl omarchy omarchy-shell omarchy-restart-shell wtype curl wget; do
  printf '#!/bin/bash\necho "forbidden %s" >> "$CALL_LOG"\nexit 99\n' "$command" > "$scratch/mock/$command"
done
chmod +x "$scratch/mock/"*
export PATH="$scratch/mock:$PATH"
# Binds atuais: os do FastWisper (não contam) e um de outro programa em SUPER+D.
cat > "$FAKE_BINDS" <<'JSON'
[
  {"modmask":8,"key":"SPACE","keycode":0,"submap":"","mouse":false,"description":"FastWisper: ditar (segurar)","dispatcher":"__lua","arg":"1"},
  {"modmask":9,"key":"SPACE","keycode":0,"submap":"","mouse":false,"description":"FastWisper: ditar sem segurar","dispatcher":"__lua","arg":"2"},
  {"modmask":72,"key":"V","keycode":0,"submap":"","mouse":false,"description":"FastWisper: colar último ditado","dispatcher":"__lua","arg":"3"},
  {"modmask":64,"key":"D","keycode":0,"submap":"","mouse":false,"description":"Abrir o lançador","dispatcher":"__lua","arg":"4"}
]
JSON
bash "$scratch/fixture/packaging/omarchy/install.sh" --no-activate --no-hub > /dev/null
installed=$XDG_DATA_HOME/fastwisper/hyprland.lua
hotkeys=$XDG_DATA_HOME/fastwisper/hotkeys.conf
grep -qxF "local hotkeys_file = \"$hotkeys\"" "$installed"
grep -qxF "local cli = \"'$HOME/.local/bin/fastwisper'\"" "$installed"
refute grep -q '@FASTWISPER_' "$installed"
fw() { "$HOME/.local/bin/fastwisper" "$@"; }
fake() { "$lua" "$harness" "$installed" "$@"; }
expect() { # expect <saída esperada> passo...
  local want=$1; shift
  local got
  got=$(fake "$@")
  [[ "$got" == "$want" ]] || { printf 'Hyprland falso: %s\nesperado:\n%s\nobtido:\n%s\n' "$*" "$want" "$got" >&2; exit 1; }
}
ctl="exec '$HOME/.local/bin/fastwisper' ctl"
defaults="bind ALT + SHIFT + SPACE = FastWisper: ditar sem segurar
bind ALT + SPACE = FastWisper: ditar (segurar)
bind SUPER + ALT + V = FastWisper: colar último ditado"

# 1. Sem arquivo: os padrões de sempre; soltar Space, Alt esquerdo ou direito encerra.
[[ ! -e "$hotkeys" ]]
expect "$defaults" binds
expect "$ctl press
$ctl release" press 'ALT + SPACE' key 65 0
expect "$ctl press
$ctl release" press 'ALT + SPACE' key 108 0
expect "$ctl press
$ctl release
watchers 0" press 'ALT + SPACE' key 64 0 key 64 0 key 65 0 watchers
# Shift ou outra tecla soltos não encerram; Esc cancela (e segue para o app).
expect "$ctl press
watchers 1
$ctl cancel
watchers 0" press 'ALT + SPACE' key 50 0 key 40 0 watchers key 9 1 watchers
expect "$ctl toggle
$ctl cancel
$ctl paste-last" press 'ALT + SHIFT + SPACE' key 9 1 press 'SUPER + ALT + V'
# Config padrão sem arquivo: reload da config não grava nem recarrega o Hyprland.
fw config reload > "$scratch/out" 2>&1 || true
[[ ! -e "$hotkeys" ]]
refute grep -q 'hyprctl reload' "$CALL_LOG" 2>/dev/null

# 2. Conflito com bind de outro programa: sem terminal e sem --yes, nada muda.
if fw config set hotkey.dictate 'super + d' > "$scratch/out" 2>&1; then
  echo 'conflito gravado sem confirmação' >&2; exit 1
fi
grep -qF 'is already used by "Abrir o lançador"' "$scratch/out"
grep -qF -- '--yes' "$scratch/out"
refute grep -q 'dictate' "$XDG_CONFIG_HOME/fastwisper/config.toml"
[[ ! -e "$hotkeys" ]]
refute grep -q 'hyprctl reload' "$CALL_LOG"
# Com --yes: grava canônico, gera o arquivo e recarrega o Hyprland uma vez.
fw config set --yes hotkey.dictate 'super + d' > "$scratch/out" 2>&1
grep -qxF 'dictate = "SUPER+D"' "$XDG_CONFIG_HOME/fastwisper/config.toml"
grep -qF 'Hyprland reloaded' "$scratch/out"
[[ $(grep -c 'hyprctl reload' "$CALL_LOG") == 1 ]]
grep -qxF 'dictate SUPER+D 40 133 134' "$hotkeys"
[[ $(head -n1 "$hotkeys") == '# FastWisper: atalhos gerados da config ([hotkey]).' ]]
[[ $(find "$XDG_DATA_HOME/fastwisper" -maxdepth 1 -name 'hotkeys*' | wc -l) == 1 ]]
expect "bind ALT + SHIFT + SPACE = FastWisper: ditar sem segurar
bind SUPER + ALT + V = FastWisper: colar último ditado
bind SUPER + D = FastWisper: ditar (segurar)" binds
# Soltura: a tecla (D), Super esquerdo ou direito; Alt (fora do atalho) não.
expect "$ctl press
$ctl release" press 'SUPER + D' key 40 0
expect "$ctl press
$ctl release" press 'SUPER + D' key 133 0
expect "$ctl press
watchers 1
$ctl release" press 'SUPER + D' key 64 0 key 108 0 key 65 0 watchers key 134 0
# 3. Recarga com gravação segurada (ex.: atalho mudado no meio): cancela, nunca cola.
expect "$ctl press
$ctl cancel
watchers 0" press 'SUPER + D' reload watchers
# Mesma config de novo: nada a gravar, sem recarga extra.
fw config set --yes hotkey.dictate SUPER+D > /dev/null
[[ $(grep -c 'hyprctl reload' "$CALL_LOG") == 1 ]]

# 4. Conflito entre os três: recusado, nada muda.
cp "$hotkeys" "$scratch/hotkeys-before"
cp "$XDG_CONFIG_HOME/fastwisper/config.toml" "$scratch/config-before"
for bad in 'hotkey.hands_free SUPER+D' 'hotkey.paste_last super+d' 'hotkey.dictate SPACE' 'hotkey.dictate SHIFT+SPACE' 'hotkey.dictate ALT+ESC' 'hotkey.dictate ALT+D+E'; do
  # shellcheck disable=SC2086
  if fw config set --yes $bad > "$scratch/out" 2>&1; then echo "aceito: $bad" >&2; exit 1; fi
  cmp "$scratch/config-before" "$XDG_CONFIG_HOME/fastwisper/config.toml"
  cmp "$scratch/hotkeys-before" "$hotkeys"
done
grep -qF 'modifier other than Shift' <(fw config set hotkey.dictate SHIFT+SPACE 2>&1 || true)
grep -qF 'use the same shortcut' <(fw config set hotkey.hands_free SUPER+D 2>&1 || true)
[[ $(grep -c 'hyprctl reload' "$CALL_LOG") == 1 ]]
# Trocar dois atalhos de uma vez vale (validados juntos).
fw config set hotkey.dictate ALT+SHIFT+SPACE hotkey.hands_free ALT+SPACE > /dev/null
grep -qxF 'dictate ALT+SHIFT+SPACE 65 64 108 50 62' "$hotkeys"
grep -qxF 'hands_free ALT+SPACE' "$hotkeys"
expect "$ctl press
$ctl release" press 'ALT + SHIFT + SPACE' key 62 0
expect "$ctl toggle" press 'ALT + SPACE'

# 5. Config editada à mão + `config reload`: regera e recarrega (daemon parado não impede).
sed -i 's/^dictate = .*/dictate = "CTRL+ALT+F9"/; s/^hands_free = .*/hands_free = "ALT+SHIFT+SPACE"/' "$XDG_CONFIG_HOME/fastwisper/config.toml"
reloads=$(grep -c 'hyprctl reload' "$CALL_LOG")
fw config reload > "$scratch/out" 2>&1 || true
grep -qxF 'dictate CTRL+ALT+F9 75 37 105 64 108' "$hotkeys"
[[ $(grep -c 'hyprctl reload' "$CALL_LOG") == $((reloads + 1)) ]]
expect "$ctl press
$ctl release" press 'CTRL + ALT + F9' key 105 0
# --config fora do caminho padrão não mexe no arquivo da sessão.
printf '[hotkey]\ndictate = "SUPER+F8"\n' > "$scratch/other.toml"
fw config set --config "$scratch/other.toml" hotkey.dictate SUPER+F7 > "$scratch/out"
grep -qF 'outside the default path' "$scratch/out"
grep -qxF 'dictate CTRL+ALT+F9 75 37 105 64 108' "$hotkeys"

# 6. CLI: list, check (conflito, duplicado), keys e apply.
fw hotkeys list > "$scratch/out"
grep -qxF 'Dictation shortcut: Ctrl + Alt + F9 (hotkey.dictate)  # default: ALT+SPACE' "$scratch/out"
# Saída num arquivo, nunca `fw ... | grep -q`: o grep sai no primeiro acerto,
# a escrita seguinte da CLI dá EPIPE e, com pipefail, o teste falha ao acaso.
fw hotkeys list --json > "$scratch/out"
grep -qF '"hotkey":"CTRL+ALT+F9"' "$scratch/out"
fw hotkeys check dictate super+d > "$scratch/out"
grep -qxF 'Conflict: Abrir o lançador already uses SUPER+D.' "$scratch/out"
fw hotkeys check paste_last ctrl+alt+f9 > "$scratch/out" 2>&1 && { echo 'duplicado aceito' >&2; exit 1; }
grep -qF 'is already the shortcut for Dictation shortcut' "$scratch/out"
fw hotkeys keys > "$scratch/out"
grep -qF 'SPACE' "$scratch/out"
rm "$hotkeys"
fw hotkeys apply --no-reload > /dev/null
grep -qxF 'dictate CTRL+ALT+F9 75 37 105 64 108' "$hotkeys"

# 7. Arquivo inválido, grande demais ou com tentativa de injeção: padrões, nada executa.
expect_defaults() { expect "$defaults" binds; }
for content in \
  $'version 1\ndictate ALT+SPACE"); os.execute("touch /tmp/x") --\nhands_free ALT+SHIFT+SPACE\npaste_last SUPER+ALT+V' \
  $'version 1\ndictate SUPER+D 40 133 134\nhands_free ALT+SHIFT+SPACE\npaste_last SUPER+ALT+V\nos.execute("x")' \
  $'version 1\ndictate SHIFT+SPACE 65 50 62\nhands_free ALT+SHIFT+SPACE\npaste_last SUPER+ALT+V' \
  $'version 1\ndictate SPACE 65\nhands_free ALT+SHIFT+SPACE\npaste_last SUPER+ALT+V' \
  $'version 1\ndictate ALT+SPACE\nhands_free ALT+SHIFT+SPACE\npaste_last SUPER+ALT+V' \
  $'version 1\ndictate SUPER+D 40 99999\nhands_free ALT+SHIFT+SPACE\npaste_last SUPER+ALT+V' \
  $'version 1\ndictate SUPER+D 40\nhands_free SUPER+D\npaste_last SUPER+ALT+V' \
  $'version 1\ndictate SUPER+D 40\ndictate SUPER+E 26\nhands_free ALT+SHIFT+SPACE\npaste_last SUPER+ALT+V' \
  $'version 2\ndictate SUPER+D 40\nhands_free ALT+SHIFT+SPACE\npaste_last SUPER+ALT+V' \
  $'dictate SUPER+D 40\nhands_free ALT+SHIFT+SPACE\npaste_last SUPER+ALT+V' \
  $'version 1\ndictate SUPER+ESCAPE 9\nhands_free ALT+SHIFT+SPACE\npaste_last SUPER+ALT+V' \
  $'version 1\ndictate SUPER+FOO+D 40\nhands_free ALT+SHIFT+SPACE\npaste_last SUPER+ALT+V' \
  ''; do
  printf '%s\n' "$content" > "$hotkeys"
  expect_defaults
done
{ printf 'version 1\ndictate SUPER+D 40 133 134\nhands_free ALT+SHIFT+SPACE\npaste_last SUPER+ALT+V\n'; head -c 5000 /dev/zero | tr '\0' '#'; printf '\n'; } > "$hotkeys"
expect_defaults
# Arquivo válido escrito à mão continua valendo (o formato é o contrato).
printf 'version 1\ndictate SUPER+D 40 133 134\nhands_free ALT+SHIFT+SPACE\npaste_last SUPER+ALT+V\n' > "$hotkeys"
expect "bind ALT + SHIFT + SPACE = FastWisper: ditar sem segurar
bind SUPER + ALT + V = FastWisper: colar último ditado
bind SUPER + D = FastWisper: ditar (segurar)" binds

# 8. Desinstalar remove o arquivo gerado (só o nosso).
fw hotkeys apply --no-reload > /dev/null
bash "$scratch/fixture/packaging/omarchy/uninstall.sh" > /dev/null
[[ ! -e "$hotkeys" && ! -e "$installed" ]]
refute grep -q forbidden "$CALL_LOG"
echo 'hotkeys: PASS (padrões, soltura por tecla/modificador L/R, recarga cancela, conflitos, injeção, CLI, desinstalação)'
