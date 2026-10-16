#!/bin/bash
# Unwind all managed generations; never stop services or remove external edits.
set -euo pipefail
source "$(dirname "$0")/managed.sh"
while [[ -f "$state_root/current" ]]; do
  generation=$(cat "$state_root/current")
  restore "$generation"
  if [[ -f "$generation/parent" ]]; then
    set_current "$(cat "$generation/parent")"
  else
    rm -f "$state_root/current"
  fi
done
# Atalhos gerados da config: só o arquivo que o FastWisper escreveu.
hotkeys=$data_root/fastwisper/hotkeys.conf
if [[ -f "$hotkeys" && ! -L "$hotkeys" ]] \
  && [[ $(head -n 1 "$hotkeys") == '# FastWisper: atalhos gerados da config ([hotkey]).' ]]; then
  rm -f -- "$hotkeys"
fi
say uninstall_done
say uninstall_hint
