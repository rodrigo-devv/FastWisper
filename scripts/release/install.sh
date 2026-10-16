#!/bin/bash
# Instalador do tarball da release: instala os binários prontos de bin/, sem
# compilar. Aceita as opções de packaging/omarchy/install.sh (--offline,
# --no-activate, --no-hub, --adopt-existing).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
export FASTWISPER_BUILD_DIR="$here/bin" FASTWISPER_HUB_BUILD_DIR="$here/bin"
exec bash "$here/packaging/omarchy/install.sh" "$@"
