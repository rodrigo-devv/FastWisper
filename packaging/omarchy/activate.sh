#!/bin/bash
# Somente install.sh chama estes passos; rollback/uninstall continuam em disco.
source "$(dirname "${BASH_SOURCE[0]}")/messages.sh"
optional() {
  if ! command -v "$1" >/dev/null; then warn tool_missing_skipped "$1"; return 0; fi
  "$@" 9>&- || warn optional_failed "$*"
}
# Lê os campos string produzidos pelo setup, sem executar a config como shell.
# Strings TOML com escapes/múltiplas linhas não são adivinhadas.
config_string() {
  [[ -f "$config_root/fastwisper/config.toml" ]] || return 0
  awk -v section="$1" -v key="$2" '
    /^[[:space:]]*\[/ { s=$0; sub(/^[[:space:]]*\[/,"",s); sub(/\].*$/,"",s) }
    s==section && $0 ~ "^[[:space:]]*" key "[[:space:]]*=" {
      v=$0; sub(/^[^=]*=[[:space:]]*/,"",v)
      quote=substr(v,1,1)
      if (quote!="\"" && quote!=sprintf("%c",39)) exit
      v=substr(v,2); end=index(v,quote); if (!end) exit
      v=substr(v,1,end-1); if (index(v,"\\")) exit
      print v; exit
    }' "$config_root/fastwisper/config.toml"
}
# /proc é do kernel; não imprime ambiente nem sinaliza processos de outro runtime.
uses_default_runtime() {
  local pid=$1 proc_root=${2:-/proc} runtime=${XDG_RUNTIME_DIR:-} field
  [[ -n "$runtime" && -r "$proc_root/$pid/environ" ]] || return 1
  while IFS= read -r -d '' field; do
    [[ "$field" != XDG_RUNTIME_DIR=* ]] || { [[ "$field" == "XDG_RUNTIME_DIR=$runtime" ]]; return; }
  done < "$proc_root/$pid/environ"
  return 1
}
activate_install() {
  local cli="$bin_dir/fastwisper" device errors new_errors pill_after pill_active binds_validated=0
  device=$(config_string audio device)
  if [[ -z "${device//[[:space:]]/}" ]]; then
    "$cli" setup 9>&- || { warn setup_failed; return 1; }
  fi
  if (( offline )); then
    warn offline_asr_skipped
  else
    "$cli" models pull 9>&- || { warn asr_failed; return 1; }
  fi
  # Nuvem do Ollama conforme llm.ollama_cloud (padrão: offline). No serviço
  # do sistema o sudo pede a senha; sem terminal, só avisa como aplicar.
  "$cli" ollama apply 9>&- || warn ollama_cloud_not_applied
  if command -v systemctl >/dev/null; then
    # Exclui o MainPID supervisionado; encerra somente daemons manuais do usuário.
    local main_pid manual_pid attempt
    main_pid=$(systemctl --user show fastwisperd.service --property=MainPID --value 2>/dev/null) || main_pid=0
    [[ "$main_pid" =~ ^[0-9]+$ ]] || main_pid=0
    if command -v pgrep >/dev/null; then
      while IFS= read -r manual_pid; do
        [[ "$manual_pid" =~ ^[0-9]+$ && "$manual_pid" != "$main_pid" ]] || continue
        if ! uses_default_runtime "$manual_pid"; then
          warn manual_pid_ignored "$manual_pid"
          continue
        fi
        # env resolve kill pelo PATH, permitindo fakes sem sinais no host de teste.
        if ! env kill -TERM "$manual_pid"; then
          if pgrep -u "$(id -u)" -x fastwisperd | grep -qxF "$manual_pid"; then
            warn manual_stop_failed; return 1
          fi
        fi
        for attempt in {1..50}; do
          if ! pgrep -u "$(id -u)" -x fastwisperd | grep -qxF "$manual_pid"; then break; fi
          sleep 0.1
        done
        if pgrep -u "$(id -u)" -x fastwisperd | grep -qxF "$manual_pid"; then
          warn manual_still_running; return 1
        fi
      done < <(pgrep -u "$(id -u)" -x fastwisperd || true)
    else warn pgrep_missing; fi
    optional systemctl --user daemon-reload
    optional systemctl --user enable fastwisperd.service
    optional systemctl --user restart fastwisperd.service
  else warn systemctl_missing; fi
  # Atalhos da config ([hotkey]) para o Lua; a recarga abaixo os aplica.
  "$cli" hotkeys apply --no-reload 9>&- || warn hotkeys_not_applied
  if [[ -f "$bindings" ]] && command -v hyprctl >/dev/null; then
    if (( errors_before_valid )); then
      if hyprctl reload 9>&- && errors=$(hyprctl configerrors 2>&1); then
        new_errors=$(comm -13 <(printf '%s\n' "$errors_before" | sed '/^[[:space:]]*$/d' | sort -u) \
          <(printf '%s\n' "$errors" | sed '/^[[:space:]]*$/d' | sort -u))
        if [[ -n "$new_errors" ]]; then
          warn hypr_new_errors "$new_errors"
          local previous="$generation"
          if [[ ! -f "$previous/entries" && -f "$state_root/current" ]]; then previous=$(cat "$state_root/current"); fi
          while [[ -f "$previous/entries" ]]; do
            restore "$previous" block
            [[ -f "$previous/parent" ]] || break
            previous=$(cat "$previous/parent")
          done
          optional hyprctl reload
          return 1
        fi
        binds_validated=1
      else warn hypr_unavailable; fi
    else warn hypr_unavailable; fi
  else warn hypr_missing; fi
  # Pílula: plugin `panel` sem lugar na barra; habilitado = listado em `.plugins`.
  # Quem não quer a pílula usa `[overlay] enabled = false`.
  if ! command -v jq >/dev/null; then
    warn jq_missing
  elif [[ -f "$pill_dir/Pill.qml" ]] \
    && ! jq -e 'any(.plugins[]?; (.id? // .) == "fastwisper.pill")' \
      "$config_root/omarchy/shell.json" >/dev/null 2>&1; then
    optional omarchy-shell shell rescanPlugins
    if command -v omarchy >/dev/null; then optional omarchy plugin enable fastwisper.pill
    else warn tool_missing_skipped omarchy; fi
  fi
  # O shell não relê QML/JS de plugin carregado: pílula mudada reinicia.
  # O hash cobre o QML e a lógica importada por ele.
  local restart=0 rescan=0
  pill_after=''
  if [[ -f "$pill_dir/Pill.qml" ]]; then
    pill_after=$(cat "$pill_dir/Pill.qml" "$pill_dir/PillLogic.js" 2>/dev/null | sha256sum | cut -d' ' -f1)
  fi
  pill_active=''
  if [[ -f "$state_root/active-pill-qml" && ! -L "$state_root/active-pill-qml" ]]; then pill_active=$(cat "$state_root/active-pill-qml"); fi
  if [[ -n "$pill_after" ]]; then
    if [[ -n "$pill_active" && "$pill_active" != "$pill_after" ]]; then restart=1
    elif [[ -z "$pill_active" ]]; then rescan=1; fi
  fi
  if (( restart )); then
    if ! command -v omarchy-restart-shell >/dev/null || ! omarchy-restart-shell 9>&-; then
      warn shell_restart_failed
      return 1
    fi
  elif (( rescan )); then
    if ! command -v omarchy-shell >/dev/null || ! omarchy-shell shell rescanPlugins 9>&-; then
      warn shell_rescan_failed
      return 1
    fi
  fi
  if [[ -n "$pill_after" && "$pill_active" != "$pill_after" && ! -L "$state_root/active-pill-qml" ]]; then
    local pill_tmp
    pill_tmp=$(mktemp "$state_root/.active-pill-qml.XXXXXX")
    printf '%s\n' "$pill_after" > "$pill_tmp"
    mv -f "$pill_tmp" "$state_root/active-pill-qml"
  fi
  (( binds_validated )) || warn binds_not_validated
  bar_plugin_step
}
# Widget da barra: plugin separado do Omarchy (rede: só com --bar-plugin e
# sem --offline). Nunca falha a instalação; arquivos legados ficam intactos.
bar_plugin_step() {
  local url=https://github.com/s3xz/omarchy-fastwisper.git
  local legacy=$config_root/omarchy/plugins/fastwisper.dictation
  if [[ -f "$config_root/omarchy/plugins/io.github.s3xz.fastwisper/manifest.json" ]]; then
    return 0
  elif (( ! ${bar_plugin:-0} )); then
    if [[ -f "$legacy/manifest.json" ]]; then warn bar_plugin_migrate "$url"
    else say bar_plugin_hint "$url"; fi
  elif (( offline )); then
    warn bar_plugin_offline "$url"
  elif ! command -v omarchy >/dev/null; then
    warn tool_missing_skipped omarchy
  elif omarchy plugin add "$url" --enable --yes 9>&-; then
    # O widget legado no layout ficaria ao lado do novo.
    if command -v jq >/dev/null && jq -e 'any((.bar.layout // {} | .[] | .[]); .id == "fastwisper.dictation")' \
      "$config_root/omarchy/shell.json" >/dev/null 2>&1; then
      optional omarchy plugin disable fastwisper.dictation
    fi
  else
    warn optional_failed "omarchy plugin add $url --enable --yes"
  fi
}
