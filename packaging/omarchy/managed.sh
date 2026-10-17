#!/bin/bash
# Shared transaction engine. No service, desktop or network commands.
set -euo pipefail
umask 077
source "$(dirname "${BASH_SOURCE[0]}")/messages.sh"
config_root=${XDG_CONFIG_HOME:-$HOME/.config}
data_root=${XDG_DATA_HOME:-$HOME/.local/share}
state_root=${XDG_STATE_HOME:-$HOME/.local/state}/fastwisper/packaging
bin_dir=$HOME/.local/bin
bindings=$config_root/hypr/bindings.lua
for path in "$config_root" "$data_root" "$state_root" "$bin_dir"; do
  [[ "$path" == /* ]] || { say_err absolute_path_required "$path"; exit 1; }
done
case "$state_root$bin_dir$config_root$data_root" in
  *$'\n'*) say_err newline_path_refused; exit 1 ;;
esac
ancestor=$state_root
while [[ "$ancestor" != / ]]; do
  [[ ! -L "$ancestor" ]] || { say_err symlink_dir_refused "$ancestor"; exit 1; }
  ancestor=$(dirname "$ancestor")
done
# active-qml: hash do antigo QML da barra, sem uso; segue recusado como symlink.
for path in lock current pending active-qml active-pill-qml hub-status; do
  [[ ! -L "$state_root/$path" ]] || { say_err symlink_state_refused "$path"; exit 1; }
done
mkdir -p "$state_root"
chmod 0700 "$state_root"
exec 9>>"$state_root/lock"
flock -n 9 || { say_err lock_busy; exit 1; }
set_current() {
  local pointer
  pointer=$(mktemp "$state_root/.current.XXXXXX")
  printf '%s\n' "$1" > "$pointer"
  mv -f -- "$pointer" "$state_root/current"
}
# Use the same ancestor policy for file writes, binding blocks and restoration.
check_ancestors() {
  local ancestor
  ancestor=$(dirname "$1")
  while [[ "$ancestor" != / ]]; do
    [[ ! -L "$ancestor" ]] || { say_err symlink_dir_refused "$ancestor"; return 1; }
    ancestor=$(dirname "$ancestor")
  done
}
# Restore only content still owned by this transaction; external edits win.
restore() {
  local generation=$1 index path kind current expected tmp block
  [[ -f "$generation/entries" ]] || return 0
  while IFS= read -r index; do
    path=$(cat "$generation/$index/path")
    if ! check_ancestors "$path" 2>/dev/null; then
      say_err preserved_symlink_dir "$path"
      continue
    fi
    kind=$(cat "$generation/$index/kind")
    [[ -z ${2:-} || "$kind" == "$2" ]] || continue
    if [[ $kind == block ]]; then
      [[ -f "$path" && ! -L "$path" ]] || continue
      if [[ -f "$generation/$index/mode" ]] && [[ $(stat -c '%a' "$path") != $(cat "$generation/$index/mode") ]]; then
        say_err preserved_external_mode "$path"
        continue
      fi
      block=$(cat "$generation/$index/after")
      block+=$'\n'
      current=$(cat "$path")
      if [[ "$current"$'\n' == *"$block"* ]]; then
        tmp=$(mktemp "${path}.XXXXXX")
        # Preserve every byte outside the exact owned block, including EOF blanks.
        # ENVIRON, not -v: awk would decode the backslashes of an escaped path.
        FASTWISPER_BLOCK=$block awk 'BEGIN { RS="\0"; ORS=""; block=ENVIRON["FASTWISPER_BLOCK"] }
          { start=index($0,block); if (start) print substr($0,1,start-1) substr($0,start+length(block)); else print }
        ' "$path" > "$tmp"
        chmod --reference="$path" "$tmp"
        mv -f -- "$tmp" "$path"
      fi
      continue
    fi
    [[ -f "$path" && ! -L "$path" ]] || continue
    expected=$(sha256sum "$generation/$index/after" | cut -d' ' -f1)
    current=$(sha256sum "$path" | cut -d' ' -f1)
    if [[ "$current" != "$expected" ]] || [[ $(stat -c '%a' "$path") != $(stat -c '%a' "$generation/$index/after") ]]; then
      say_err preserved_external_edit "$path"
      continue
    fi
    if [[ -f "$generation/$index/before" ]]; then
      tmp=$(mktemp "${path}.XXXXXX")
      cp -p -- "$generation/$index/before" "$tmp"
      mv -f -- "$tmp" "$path"
    else
      rm -f -- "$path"
    fi
  done < <(tac "$generation/entries")
}
managed_file() {
  local source=$1 path=$2 mode=$3 index tmp previous previous_index found=0
  check_ancestors "$path" || return 1
  # On upgrade, don't overwrite a file changed since the previous install.
  if [[ -f "$state_root/current" ]]; then
    previous=$(cat "$state_root/current")
    while [[ -n "$previous" ]]; do
    while IFS= read -r previous_index; do
      if [[ $(cat "$previous/$previous_index/path") == "$path" ]]; then
        found=1
        if [[ ! -f "$path" ]] || ! cmp -s "$path" "$previous/$previous_index/after" \
          || [[ $(stat -c '%a' "$path") != $(stat -c '%a' "$previous/$previous_index/after") ]]; then
          say_err preserved_later_choice "$path"
          return
        fi
        break
      fi
    done < "$previous/entries"
    (( found )) && break
    if [[ -f "$previous/parent" ]]; then previous=$(cat "$previous/parent"); else previous=''; fi
    done
  fi
  if (( ! found && ! ${adopt_existing:-0} )) && [[ -e "$path" || -L "$path" ]]; then
    say_err preserved_preexisting "$path"
    return
  fi
  [[ ! -L "$path" ]] || { say_err symlink_refused "$path"; return 1; }
  # Evita writes, mudanças de mtime e gerações vazias em reinstalações.
  if (( found || ! ${adopt_existing:-0} )) && [[ -f "$path" ]] && cmp -s "$source" "$path" \
    && [[ $(stat -c '%a' "$path") == "${mode#0}" ]]; then return; fi
  index=$(wc -l < "$generation/entries")
  mkdir "$generation/$index"
  printf '%s\n' "$path" > "$generation/$index/path"
  echo file > "$generation/$index/kind"
  [[ ! -f "$path" ]] || cp -p -- "$path" "$generation/$index/before"
  install -m "$mode" "$source" "$generation/$index/after"
  printf '%s\n' "$index" >> "$generation/entries"
  mkdir -p "$(dirname "$path")"
  tmp=$(mktemp "${path}.XXXXXX")
  install -m "$mode" "$source" "$tmp"
  mv -f -- "$tmp" "$path"
}
# Recover an interrupted installation before starting another operation.
if [[ -f "$state_root/pending" ]]; then
  pending=$(cat "$state_root/pending")
  if [[ ! -f "$state_root/current" ]] || [[ $(cat "$state_root/current") != "$pending" ]]; then
    restore "$pending"
    rm -rf -- "$pending"
  fi
  rm -f "$state_root/pending"
fi
