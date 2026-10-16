#!/bin/bash
# Chamado pelo contrato principal (`source` no fim do contract.sh), sempre em
# HOME/XDG e PATH descartáveis. Rodado sozinho não teria `$scratch` nem os
# mocks e passaria sem testar nada: falha alto.
if [[ "${BASH_SOURCE[0]}" == "$0" || -z ${scratch:-} || ! -d $scratch || ! -d ${repo:-}/packaging/omarchy ]]; then
  echo 'activation.sh faz parte do contract.sh: rode bash packaging/omarchy/tests/contract.sh' >&2
  exit 1
fi
set -euo pipefail
export HOME="$scratch/automatic home"
export XDG_CONFIG_HOME="$HOME/config" XDG_DATA_HOME="$HOME/data" XDG_STATE_HOME="$HOME/state"
export CALL_LOG="$scratch/activation-calls" FAKE_CLI="$scratch/mock/fastwisper"
export SERVICE_ACTIVE=0 HYPR_ERRORS='' HYPR_NEW_ERRORS='' HYPR_RC=0
export XDG_RUNTIME_DIR="$scratch/runtime"
XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" sleep 120 >/dev/null 2>&1 &
manual_test_pid=$!
trap 'kill "$manual_test_pid" 2>/dev/null || true; rm -rf -- "$scratch"' EXIT
export MANUAL_TEST_PID=$manual_test_pid
export MANUAL_PID_FILE="$scratch/manual-pid"
echo "$manual_test_pid" > "$MANUAL_PID_FILE"
mkdir -p "$XDG_CONFIG_HOME/hypr"
printf '%s\n' '-- binds externos' > "$XDG_CONFIG_HOME/hypr/bindings.lua"
cat > "$FAKE_CLI" <<'MOCK'
#!/bin/bash
printf 'fastwisper %s\n' "$*" >> "$CALL_LOG"
[[ ${CLI_FAIL:-} != "$1" ]] || exit 42
case "$*" in
  setup)
    mkdir -p "$XDG_CONFIG_HOME/fastwisper"
    printf '[audio]\ndevice = "mic genérico"\n[asr]\nmodel = "%s/fastwisper/models/ggml-small-q5_1.bin"\n' "$XDG_DATA_HOME" > "$XDG_CONFIG_HOME/fastwisper/config.toml" ;;
  'models pull')
    [[ ! -f "$XDG_DATA_HOME/fastwisper/models/ggml-small-q5_1.bin" ]] || exit 0
    mkdir -p "$XDG_DATA_HOME/fastwisper/models"
    echo 'modelo fake' > "$XDG_DATA_HOME/fastwisper/models/ggml-small-q5_1.bin" ;;
  'ollama apply') ;;
  'hotkeys apply --no-reload') ;;
  *) exit 99 ;;
esac
MOCK
cat > "$scratch/mock/systemctl" <<'MOCK'
#!/bin/bash
printf 'systemctl %s\n' "$*" >> "$CALL_LOG"
if [[ "$*" == *--property=MainPID* ]]; then echo 313131; fi
MOCK
cat > "$scratch/mock/hyprctl" <<'MOCK'
#!/bin/bash
printf 'hyprctl %s\n' "$*" >> "$CALL_LOG"
if [[ "$1" == reload ]]; then touch "$CALL_LOG.reload"; fi
if [[ "$1" == configerrors ]]; then
  [[ "$HYPR_RC" == 0 ]] || exit "$HYPR_RC"
  printf '%s' "$HYPR_ERRORS"
  if [[ ${HYPR_AUTORELOAD:-0} == 1 ]] && grep -q 'BEGIN fastwisper' "$XDG_CONFIG_HOME/hypr/bindings.lua"; then
    printf '%s' "$HYPR_NEW_ERRORS"
  fi
  if [[ ${HYPR_AUTORELOAD:-0} != 1 && -f "$CALL_LOG.reload" ]]; then
    printf '%s' "$HYPR_NEW_ERRORS"
  fi
fi
MOCK
cat > "$scratch/mock/omarchy" <<'MOCK'
#!/bin/bash
printf 'omarchy %s\n' "$*" >> "$CALL_LOG"
if [[ "$*" == *'--after omarchy.tray' && ${TRAY_FAIL:-0} == 1 ]]; then exit 1; fi
shell_json="$XDG_CONFIG_HOME/omarchy/shell.json"
mkdir -p "$XDG_CONFIG_HOME/omarchy"
[[ -f "$shell_json" ]] || echo '{}' > "$shell_json"
case "$*" in
  'plugin enable fastwisper.dictation --section right --after omarchy.tray'|'plugin enable fastwisper.dictation --section right')
    filter='.bar.layout.right = [{"id":"omarchy.tray"},{"id":"fastwisper.dictation"}] | .plugins = ((.plugins // []) + ["fastwisper.dictation"])' ;;
  'plugin enable fastwisper.pill') filter='.plugins = ((.plugins // []) + [{"id":"fastwisper.pill"}])' ;;
  *) exit 99 ;;
esac
jq "$filter" "$shell_json" > "$shell_json.tmp" && mv "$shell_json.tmp" "$shell_json"
MOCK
for command in pkill pgrep omarchy-shell omarchy-restart-shell; do
  printf '#!/bin/bash\nprintf "%s %%s\\n" "$*" >> "$CALL_LOG"\n' "$command" > "$scratch/mock/$command"
done
cat >> "$scratch/mock/pgrep" <<'MOCK'
[[ ! -f "$MANUAL_PID_FILE" ]] || cat "$MANUAL_PID_FILE"
if [[ "$SERVICE_ACTIVE" == 1 ]]; then echo 313131; fi
MOCK
cat > "$scratch/mock/kill" <<'MOCK'
#!/bin/bash
printf 'kill %s\n' "$*" >> "$CALL_LOG"
[[ "$*" == "-TERM $MANUAL_TEST_PID" ]] || exit 99
rm "$MANUAL_PID_FILE"
[[ ${KILL_RACE:-0} == 0 ]] || exit 1
MOCK
chmod +x "$scratch/mock/"*
# Candidato chama o fake de CLI no PATH; nada executa o core ou acessa hardware.
printf '#!/bin/bash\nexec "$FAKE_CLI" "$@"\n' > "$FASTWISPER_BUILD_DIR/fastwisper"
printf '#!/bin/bash\nexit 99\n' > "$FASTWISPER_BUILD_DIR/fastwisperd"
chmod +x "$FASTWISPER_BUILD_DIR/"*
auto() { rm -f "$CALL_LOG.reload"; bash "$repo/packaging/omarchy/install.sh" "$@" > /dev/null; }
# Opt-out completo não executa nenhum comando externo.
auto --no-activate
[[ ! -e "$CALL_LOG" && ! -e "$XDG_CONFIG_HOME/fastwisper/config.toml" ]]
run uninstall
# Instalação nova: setup, perfil CPU, serviço, binds, rescan e barra.
auto
grep -qxF 'fastwisper models pull' "$CALL_LOG"
# Atalhos da config gerados antes da recarga do Hyprland, que os aplica.
grep -qxF 'fastwisper hotkeys apply --no-reload' "$CALL_LOG"
[[ $(grep -nxF 'fastwisper hotkeys apply --no-reload' "$CALL_LOG" | cut -d: -f1) -lt $(grep -nxF 'hyprctl reload' "$CALL_LOG" | head -n1 | cut -d: -f1) ]]
grep -qxF "kill -TERM $manual_test_pid" "$CALL_LOG"
grep -qxF 'systemctl --user restart fastwisperd.service' "$CALL_LOG"
refute grep -q 'omarchy-restart-shell' "$CALL_LOG"
# Segunda execução: arquivos/ledger/mtimes iguais, sem setup/pull/enable plugin/restart shell.
cp "$XDG_STATE_HOME/fastwisper/packaging/current" "$scratch/current"
find "$HOME" -type f -printf '%p %T@ %m\n' | sort > "$scratch/files-before"
: > "$CALL_LOG"
export SERVICE_ACTIVE=1
auto
find "$HOME" -type f -printf '%p %T@ %m\n' | sort > "$scratch/files-after"
cmp "$scratch/files-before" "$scratch/files-after"
cmp "$scratch/current" "$XDG_STATE_HOME/fastwisper/packaging/current"
refute grep -Eq 'fastwisper setup|kill|omarchy' "$CALL_LOG"
grep -qxF 'systemctl --user restart fastwisperd.service' "$CALL_LOG"
# Daemon manual coexistindo com serviço ativo: TERM só no PID manual.
echo "$manual_test_pid" > "$MANUAL_PID_FILE"
: > "$CALL_LOG"
auto
grep -qxF "kill -TERM $manual_test_pid" "$CALL_LOG"
refute grep -q 'kill -TERM 313131' "$CALL_LOG"
# Configerrors também desfaz o bloco gerenciado numa repetição sem writes.
export HYPR_NEW_ERRORS='erro na repetição'
if auto 2> "$scratch/repeat-errors"; then exit 1; fi
refute grep -q 'BEGIN fastwisper' "$XDG_CONFIG_HOME/hypr/bindings.lua"
export HYPR_NEW_ERRORS=''
auto
[[ $(grep -c 'BEGIN fastwisper' "$XDG_CONFIG_HOME/hypr/bindings.lua") == 1 ]]
# Upgrade somente binário não exige restart shell; mudança QML exige uma vez.
echo '# nova versão' >> "$FASTWISPER_BUILD_DIR/fastwisper"
: > "$CALL_LOG"
auto
refute grep -q 'omarchy-restart-shell' "$CALL_LOG"
echo '// upgrade automático' >> "$repo/packaging/omarchy/shell-plugin/FastWisper.qml"
: > "$CALL_LOG"
auto
[[ $(grep -c 'omarchy-restart-shell' "$CALL_LOG") == 1 ]]
: > "$CALL_LOG"
run rollback
[[ ! -s "$CALL_LOG" ]]
refute grep -q 'upgrade automático' "$XDG_CONFIG_HOME/omarchy/plugins/fastwisper.dictation/FastWisper.qml"
# Offline mantém setup e ativação, mas nenhum download.
rm "$XDG_DATA_HOME/fastwisper/models/ggml-small-q5_1.bin"
printf '[audio]\ndevice = ""\n' > "$XDG_CONFIG_HOME/fastwisper/config.toml"
: > "$CALL_LOG"
auto --offline
grep -qxF 'fastwisper setup' "$CALL_LOG"
refute grep -q 'models pull' "$CALL_LOG"
[[ ! -e "$XDG_DATA_HOME/fastwisper/models/ggml-small-q5_1.bin" ]]
# Plugin listado mas sem widget na barra deve ser habilitado.
printf '{"plugins":["fastwisper.dictation"],"bar":{"layout":{"right":[]}}}\n' > "$XDG_CONFIG_HOME/omarchy/shell.json"
: > "$CALL_LOG"
auto
grep -q '^omarchy plugin enable ' "$CALL_LOG"
# Configerrors remove somente o bloco novo e mantém payload e binds externos.
run uninstall
export HYPR_NEW_ERRORS='erro sintético'
: > "$CALL_LOG"
if auto; then exit 1; fi
refute grep -q 'BEGIN fastwisper' "$XDG_CONFIG_HOME/hypr/bindings.lua"
grep -qxF -- '-- binds externos' "$XDG_CONFIG_HOME/hypr/bindings.lua"
[[ -x "$HOME/.local/bin/fastwisper" ]]
export HYPR_NEW_ERRORS=''
# Autoreload já vê o erro assim que o bloco é gravado.
export HYPR_AUTORELOAD=1 HYPR_NEW_ERRORS='erro autoreload'
if auto; then exit 1; fi
refute grep -q 'BEGIN fastwisper' "$XDG_CONFIG_HOME/hypr/bindings.lua"
export HYPR_AUTORELOAD=0 HYPR_NEW_ERRORS=''
# Edições de modo e QML prevalecem inclusive no upgrade automático.
chmod 0600 "$XDG_CONFIG_HOME/omarchy/plugins/fastwisper.dictation/FastWisper.qml"
echo '// escolha externa' >> "$XDG_CONFIG_HOME/omarchy/plugins/fastwisper.dictation/FastWisper.qml"
: > "$CALL_LOG"
auto
grep -q 'omarchy-restart-shell' "$CALL_LOG"
run rollback
[[ $(stat -c '%a' "$XDG_CONFIG_HOME/omarchy/plugins/fastwisper.dictation/FastWisper.qml") == 600 ]]
grep -q 'escolha externa' "$XDG_CONFIG_HOME/omarchy/plugins/fastwisper.dictation/FastWisper.qml"
# Ausência das ferramentas desktop/systemd não impede a cópia.
mkdir "$scratch/minimal-path"
for executable in /usr/bin/*; do
  [[ -f "$executable" && -x "$executable" ]] || continue
  case "${executable##*/}" in systemctl|hyprctl|omarchy*|pkill|pgrep) continue ;; esac
  ln -s "$executable" "$scratch/minimal-path/${executable##*/}"
done
rm "$XDG_CONFIG_HOME/omarchy/shell.json"
rm "$XDG_CONFIG_HOME/hypr/bindings.lua"
PATH="$scratch/minimal-path" auto --offline 2> "$scratch/missing-warnings"
grep -q 'systemctl not found' "$scratch/missing-warnings"
grep -q 'hyprctl or Lua bindings not found' "$scratch/missing-warnings"
grep -q 'omarchy-shell not found' "$scratch/missing-warnings"
grep -q 'omarchy not found' "$scratch/missing-warnings"
[[ -x "$HOME/.local/bin/fastwisperd" ]]
# Falhas de preparação não ativam o serviço nem desfazem payload copiado.
rm "$XDG_DATA_HOME/fastwisper/models/ggml-small-q5_1.bin"
: > "$CALL_LOG"
if CLI_FAIL=models auto; then echo 'download malsucedido aceito' >&2; exit 1; fi
refute grep -q '^systemctl' "$CALL_LOG"
[[ -x "$HOME/.local/bin/fastwisperd" ]]
printf '[audio]\ndevice = ""\n' > "$XDG_CONFIG_HOME/fastwisper/config.toml"
: > "$CALL_LOG"
if CLI_FAIL=setup auto; then echo 'setup malsucedido aceito' >&2; exit 1; fi
refute grep -q '^systemctl' "$CALL_LOG"
refute grep -q 'pull-llm\|forbidden' "$CALL_LOG"

# Erros preexistentes não removem binds; indisponibilidade 1/4 preserva o bloco.
printf '%s\n' '-- binds externos' > "$XDG_CONFIG_HOME/hypr/bindings.lua"
printf '[audio]\ndevice = "mic"\n' > "$XDG_CONFIG_HOME/fastwisper/config.toml"
export HYPR_ERRORS='erro anterior' HYPR_NEW_ERRORS=''
auto --offline
for rc in 1 4; do
  export HYPR_RC=$rc
  auto --offline 2> "$scratch/unvalidated"
  grep -q 'BEGIN fastwisper' "$XDG_CONFIG_HOME/hypr/bindings.lua"
  grep -q 'NOT validated' "$scratch/unvalidated"
done
export HYPR_RC=0 HYPR_ERRORS=''
# QML copiado sem ativar ainda exige restart na próxima ativação.
echo '// novo QML pendente' >> "$repo/packaging/omarchy/shell-plugin/FastWisper.qml"
# Remove a edição externa deste cenário anterior para um payload novamente gerenciado.
run uninstall
rm -f "$XDG_CONFIG_HOME/omarchy/plugins/fastwisper.dictation/FastWisper.qml"
auto --offline
echo '// segundo QML pendente' >> "$repo/packaging/omarchy/shell-plugin/FastWisper.qml"
auto --no-activate
: > "$CALL_LOG"
auto --offline
grep -q 'omarchy-restart-shell' "$CALL_LOG"
# Ausência da tray tenta habilitar sem --after.
printf '{"bar":{"layout":{"right":[]}}}' > "$XDG_CONFIG_HOME/omarchy/shell.json"
TRAY_FAIL=1 auto --offline
grep -qxF 'omarchy plugin enable fastwisper.dictation --section right' "$CALL_LOG"
# Corrida no TERM não aborta se o PID já saiu da lista.
echo "$manual_test_pid" > "$MANUAL_PID_FILE"
KILL_RACE=1 auto --offline
# Runtime temporário não é candidato ao encerramento.
source "$repo/packaging/omarchy/activate.sh"
mkdir -p "$scratch/proc/123" "$scratch/proc/124"
printf 'XDG_RUNTIME_DIR=%s\0' "$XDG_RUNTIME_DIR" > "$scratch/proc/123/environ"
printf 'XDG_RUNTIME_DIR=/tmp/test-runtime\0' > "$scratch/proc/124/environ"
uses_default_runtime 123 "$scratch/proc"
refute uses_default_runtime 124 "$scratch/proc"
# Runtime ausente: avisa sem sinalizar.
echo "$manual_test_pid" > "$MANUAL_PID_FILE"
: > "$CALL_LOG"
XDG_RUNTIME_DIR='' auto --offline 2> "$scratch/runtime-warning"
grep -q 'different, missing or unverifiable runtime' "$scratch/runtime-warning"
refute grep -q '^kill ' "$CALL_LOG"
# Rescan malsucedido permanece pendente após repetir.
run uninstall
rm -f "$XDG_STATE_HOME/fastwisper/packaging/active-qml"
rm -f "$XDG_CONFIG_HOME/omarchy/plugins/fastwisper.dictation/FastWisper.qml"
printf '#!/bin/bash\nexit 1\n' > "$scratch/mock/omarchy-shell"
for attempt in 1 2; do
  if auto --offline 2> "$scratch/rescan-warning"; then exit 1; fi
  grep -q 'shell rescan failed' "$scratch/rescan-warning"
  [[ ! -e "$XDG_STATE_HOME/fastwisper/packaging/active-qml" ]]
done
kill "$manual_test_pid"
echo 'ativação: PASS (ASR, offline, binds indisponíveis/novos/preexistentes, QML pendente, corrida PID e fallback tray)'
