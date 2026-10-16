#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/managed.sh"
[[ -f "$state_root/current" ]] || { say no_managed_install; exit 0; }
generation=$(cat "$state_root/current")
restore "$generation"
if [[ -f "$generation/parent" ]]; then
  set_current "$(cat "$generation/parent")"
else
  rm -f "$state_root/current"
fi
say rollback_done
