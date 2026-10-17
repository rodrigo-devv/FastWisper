#!/bin/bash
# Instalação automática; --no-activate mantém somente a transação de arquivos.
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
source "$(dirname "$0")/managed.sh"
# Só define funções; a ativação roda no fim, se pedida.
source "$(dirname "$0")/activate.sh"
# Caminhos entram nos templates como literais escapados (nunca por sed:
# `|`, `&` e `\` são especiais na substituição); só newline/CR é recusado.
case "$bin_dir$data_root$config_root" in
  *$'\r'*) say_err cr_path_refused; exit 1 ;;
esac
# String entre aspas duplas em Lua ou QML/JS: `\` e `"` escapados.
quoted_string() {
  local value=${1//\\/\\\\}
  printf '"%s"' "${value//\"/\\\"}"
}
# Palavra de shell entre aspas simples (o Lua monta comandos para o shell).
shell_word() {
  local quote="'\\''"
  printf "'%s'" "${1//\'/$quote}"
}
# render <template> MARCADOR VALOR...: troca literal, sem interpretar o valor.
render() {
  local template=$1; shift
  local text
  text=$(cat "$template"; printf x)
  text=${text%x}
  while (( $# )); do
    local marker=$1 value=$2 out='' rest
    shift 2
    rest=$text
    while [[ "$rest" == *"$marker"* ]]; do
      out+="${rest%%"$marker"*}$value"
      rest=${rest#*"$marker"}
    done
    text=$out$rest
  done
  printf '%s' "$text"
}
adopt_existing=0
offline=0
activate=1
hub=1
bar_plugin=0
for option in "$@"; do
  case "$option" in
    --adopt-existing) adopt_existing=1 ;;
    --offline) offline=1 ;;
    --no-activate) activate=0 ;;
    --no-hub) hub=0 ;;
    --bar-plugin) bar_plugin=1 ;;
    *) say_err install_usage; exit 2 ;;
  esac
done
build_dir=${FASTWISPER_BUILD_DIR:-$repo/target/release}
# Sem build externo, compila o código atual: nunca instala um target/release
# velho por engano (incremental; rápido quando nada mudou).
if [[ -z ${FASTWISPER_BUILD_DIR:-} ]]; then
  command -v cargo >/dev/null || { say_err cargo_missing; exit 1; }
  say building
  (cd "$repo" && cargo build --release --locked) || { say_err build_failed; exit 1; }
fi
for binary in fastwisper fastwisperd; do
  [[ -x "$build_dir/$binary" ]] || { say_err binary_missing "$build_dir/$binary"; exit 1; }
done
# Hub isolado do build Rust principal. Dependências ausentes não impedem o ditado.
hub_binary=${FASTWISPER_HUB_BUILD_DIR:-$repo/ui/src-tauri/target/release}/fastwisper-hub
# Motivo gravado em $state_root/hub-status: `fastwisper hub` sem o hub
# instalado explica por que (opt-out, dependência ausente, build falho).
hub_status=no_hub
if (( hub )); then
  hub_status=installed
  if [[ -n ${FASTWISPER_HUB_BUILD_DIR:-} ]]; then
    [[ -x "$hub_binary" ]] || { say_err hub_unavailable "$hub_binary"; hub=0; hub_status=external_build; }
  elif [[ -n ${FASTWISPER_BUILD_DIR:-} ]]; then
    say_err hub_external_build
    hub=0; hub_status=external_build
  elif ! command -v node >/dev/null || ! command -v npm >/dev/null; then
    say_err hub_missing_node
    hub=0; hub_status=missing_node
  elif ! command -v pkg-config >/dev/null || ! pkg-config --exists webkit2gtk-4.1; then
    say_err hub_missing_webkit
    hub=0; hub_status=missing_webkit
  else
    say hub_building
    hub_npm_args=(ci)
    hub_cargo_args=(--locked)
    if (( offline )); then hub_npm_args+=(--offline); hub_cargo_args+=(--offline); fi
    (cd "$repo/ui" && npm "${hub_npm_args[@]}" && npm run tauri -- build --no-bundle -- "${hub_cargo_args[@]}") || { say_err hub_build_failed; hub=0; hub_status=build_failed; }
  fi
fi
check_ancestors "$bindings"
[[ ! -L "$bindings" ]] || { say_err bindings_symlink "$bindings"; exit 1; }
legacy=0
if [[ -f "$bindings" ]] && grep -qxF -- '-- BEGIN fastwisper' "$bindings" \
  && grep -qxF -- "dofile($(quoted_string "$data_root/fastwisper/hyprland.lua"))" "$bindings" \
  && grep -qxF -- '-- END fastwisper' "$bindings"; then legacy=1; fi
if [[ ! -f "$state_root/current" ]]; then
  if (( adopt_existing && ! legacy )); then
    say_err legacy_refused
    exit 1
  fi
  adopt_existing=$legacy
fi
# O widget da barra é um plugin separado (passo opcional no fim da ativação).
# Arquivos de fastwisper.dictation de gerações antigas ficam no disco e no
# ledger: rollback/uninstall continuam desfazendo-os.
pill_dir=$config_root/omarchy/plugins/fastwisper.pill
# Atalhos da config ([hotkey]) que o bindings.lua do usuário também liga: o
# Hyprland dispara os dois binds. Só avisa; o arquivo do usuário fica intacto
# (o bloco do FastWisper e comentários não contam).
warn_hotkey_conflicts() {
  [[ -f "$bindings" ]] || return 0
  local entry action combo
  for entry in dictate:ALT+SPACE hands_free:ALT+SHIFT+SPACE paste_last:SUPER+ALT+V; do
    action=${entry%%:*}
    combo=$(config_string hotkey "$action")
    [[ -n "$combo" ]] || combo=${entry#*:}
    # Aspas simples viram duplas antes do awk (sem escapes octais: mawk/gawk).
    if tr "'" '"' < "$bindings" | awk -v want="$combo" '
      function canon(text,   n, parts, i, t, mods, key, out) {
        text = toupper(text)
        gsub(/[ \t,]+/, "+", text); gsub(/\++/, "+", text)
        sub(/^\+/, "", text); sub(/\+$/, "", text)
        n = split(text, parts, "+"); key = ""; split("", mods)
        for (i = 1; i <= n; i++) {
          t = parts[i]
          if (t == "CONTROL") t = "CTRL"
          if (t == "MOD4" || t == "WIN" || t == "META" || t == "LOGO") t = "SUPER"
          if (t == "MOD1") t = "ALT"
          if (t == "SUPER" || t == "CTRL" || t == "ALT" || t == "SHIFT") mods[t] = 1
          else key = t
        }
        out = ""
        if ("SUPER" in mods) out = out "SUPER+"
        if ("CTRL" in mods) out = out "CTRL+"
        if ("ALT" in mods) out = out "ALT+"
        if ("SHIFT" in mods) out = out "SHIFT+"
        return out key
      }
      BEGIN { want = canon(want); found = 0 }
      /^[ \t]*-- BEGIN fastwisper/ { skip = 1 }
      /^[ \t]*-- END fastwisper/ { skip = 0; next }
      skip || /^[ \t]*--/ { next }
      # hl.bind("ALT + SPACE", ...), o.bind(...) ou o estilo antigo hl.bind("ALT", "SPACE", ...).
      match($0, /(^|[^A-Za-z_])(hl|o)\.bind[a-z_]*\([ \t]*"[^"]*"([ \t]*,[ \t]*"[^"]*")?/) {
        text = substr($0, RSTART, RLENGTH); sub(/^[^(]*\(/, "", text)
        n = split(text, args, /"[ \t]*,[ \t]*"/)
        for (i = 1; i <= n; i++) gsub(/^[ \t]*"|"[ \t]*$/, "", args[i])
        combo = canon(args[1])
        if (combo ~ /\+$/ && n > 1) combo = canon(args[1] "+" args[2])
        if (combo == want) found = 1
      }
      END { exit !found }'; then
      warn hotkey_conflict "$bindings" "$combo" "$action" "$action"
    fi
  done
}
# O autoreload pode acontecer na escrita: a referência precede todo payload.
errors_before=''
errors_before_valid=0
if (( activate )); then
  if command -v hyprctl >/dev/null && errors_before=$(hyprctl configerrors 2>&1); then
    errors_before_valid=1
  fi
fi
# A generation contains both candidate and previous files; commit the pointer last.
generation=$(mktemp -d "$state_root/generation.XXXXXX")
: > "$generation/entries"
printf '%s\n' "$generation" > "$state_root/pending"
if [[ -f "$state_root/current" ]]; then cp "$state_root/current" "$generation/parent"; fi
committed=0
cleanup() {
  if (( ! committed )); then restore "$generation"; rm -rf -- "$generation"; fi
  rm -f "$state_root/pending"
}
trap cleanup EXIT
for binary in fastwisper fastwisperd; do
  managed_file "$build_dir/$binary" "$bin_dir/$binary" 0755
done
if (( hub )); then
  managed_file "$hub_binary" "$bin_dir/fastwisper-hub" 0755
  # O campo Exec usa quoting do Desktop Entry (não do shell).
  desktop_cli=${bin_dir//\\/\\\\}/fastwisper
  desktop_cli=${desktop_cli//\"/\\\"}
  desktop_cli=${desktop_cli//\$/\\\$}
  desktop_cli=${desktop_cli//\`/\\\`}
  desktop_cli=${desktop_cli//%/%%}
  desktop_template=$(cat "$repo/packaging/omarchy/fastwisper.desktop")
  printf '%s\n' "${desktop_template//@FASTWISPER_CLI@/\"$desktop_cli\"}" > "$generation/desktop"
  managed_file "$generation/desktop" "$data_root/applications/fastwisper.desktop" 0644
  # Guia local que o item Ajuda do hub abre (sem rede).
  if [[ -f "$repo/README.md" ]]; then
    managed_file "$repo/README.md" "$data_root/doc/fastwisper/README.md" 0644
  fi
fi
managed_file "$repo/packaging/systemd/fastwisperd.service" "$config_root/systemd/user/fastwisperd.service" 0644
# Os atalhos editáveis ficam num arquivo de dados gerado da config
# (`fastwisper hotkeys apply`), fora do ledger: é estado derivado da config.
render "$repo/packaging/omarchy/fastwisper.lua" \
  '"@FASTWISPER_CLI@"' "$(quoted_string "$(shell_word "$bin_dir/fastwisper")")" \
  '"@FASTWISPER_HOTKEYS@"' "$(quoted_string "$data_root/fastwisper/hotkeys.conf")" > "$generation/lua"
managed_file "$generation/lua" "$data_root/fastwisper/hyprland.lua" 0644
# Um bloco só (marcado, mesmo editado pelo usuário); binds do usuário nunca
# são alterados. O atalho do FastWisper vem da config ([hotkey]): um Alt+Space
# do usuário não impede o bloco, só gera o aviso de conflito abaixo.
if [[ -f "$bindings" ]] && ! grep -qF 'BEGIN fastwisper' "$bindings"; then
  index=$(wc -l < "$generation/entries")
  mkdir "$generation/$index"
  printf '%s\n' "$bindings" > "$generation/$index/path"
  echo block > "$generation/$index/kind"
  stat -c '%a' "$bindings" > "$generation/$index/mode"
  printf '\n-- BEGIN fastwisper\ndofile(%s)\n-- END fastwisper\n' "$(quoted_string "$data_root/fastwisper/hyprland.lua")" > "$generation/$index/after"
  printf '%s\n' "$index" >> "$generation/entries"
  tmp=$(mktemp "${bindings}.XXXXXX")
  cat "$bindings" "$generation/$index/after" > "$tmp"
  chmod --reference="$bindings" "$tmp"
  mv -f -- "$tmp" "$bindings"
fi
# Pílula de gravação (fase 2); o opt-out é `[overlay] enabled = false`.
managed_file "$repo/packaging/omarchy/pill-plugin/manifest.json" "$pill_dir/manifest.json" 0644
render "$repo/packaging/omarchy/pill-plugin/Pill.qml" '"@FASTWISPER_CLI@"' "$(quoted_string "$bin_dir/fastwisper")" > "$generation/pill-qml"
managed_file "$generation/pill-qml" "$pill_dir/Pill.qml" 0644
managed_file "$repo/packaging/omarchy/pill-plugin/PillLogic.js" "$pill_dir/PillLogic.js" 0644
# Uma repetição sem alterações não cria um degrau vazio no rollback.
if [[ ! -s "$generation/entries" ]]; then
  rm -rf -- "$generation"
else
  set_current "$generation"
fi
committed=1
# Repetição sem mudanças não reescreve o estado (idempotência byte a byte).
if [[ -L "$state_root/hub-status" || $(cat "$state_root/hub-status" 2>/dev/null) != "$hub_status" ]]; then
  hub_status_tmp=$(mktemp "$state_root/.hub-status.XXXXXX")
  printf '%s\n' "$hub_status" > "$hub_status_tmp"
  mv -f -- "$hub_status_tmp" "$state_root/hub-status"
fi
say files_installed
warn_hotkey_conflicts
if (( activate )); then
  source "$(dirname "$0")/activate.sh"
  activate_install
fi
say rollback_hint
