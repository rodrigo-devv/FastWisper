#!/bin/bash
set -euo pipefail
# Mensagens do instalador em inglês, qualquer que seja o locale do host.
unset LC_ALL; export LC_MESSAGES=C
# `! cmd` não dispara o `set -e`; a negação precisa sair explicitamente.
refute() { if "$@"; then echo "asserção negada falhou: $*" >&2; exit 1; fi; }
repo=$(cd "$(dirname "$0")/../../.." && pwd)
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
mkdir "$scratch/fixture"
cp -r "$repo/packaging" "$scratch/fixture/"
repo=$scratch/fixture
export HOME="$scratch/home with spaces"
export XDG_CONFIG_HOME="$HOME/config" XDG_DATA_HOME="$HOME/data" XDG_STATE_HOME="$HOME/state"
export FASTWISPER_BUILD_DIR="$scratch/build"
mkdir -p "$XDG_CONFIG_HOME/hypr" "$XDG_CONFIG_HOME/fastwisper" "$XDG_DATA_HOME/fastwisper/models" "$FASTWISPER_BUILD_DIR" "$scratch/mock"
export CALL_LOG="$scratch/calls"
for command in systemctl pkill pgrep kill hyprctl omarchy omarchy-shell omarchy-restart-shell curl wget; do
  printf '#!/bin/bash\necho forbidden >> "$CALL_LOG"\nexit 99\n' > "$scratch/mock/$command"
  chmod +x "$scratch/mock/$command"
done
export PATH="$scratch/mock:$PATH"
printf '%s\n' '-- native F9/Voxtype stays' > "$XDG_CONFIG_HOME/hypr/bindings.lua"
echo 'external config' > "$XDG_CONFIG_HOME/fastwisper/config.toml"
echo 'external model' > "$XDG_DATA_HOME/fastwisper/models/keep"
version() {
  for binary in fastwisper fastwisperd; do
    printf '#!/bin/bash\necho %s\n' "$1" > "$FASTWISPER_BUILD_DIR/$binary"
    chmod +x "$FASTWISPER_BUILD_DIR/$binary"
  done
  printf '\n-- version %s\n' "$1" >> "$repo/packaging/omarchy/fastwisper.lua"
  printf '\n// version %s\n' "$1" >> "$repo/packaging/omarchy/shell-plugin/FastWisper.qml"
  printf '\n# version %s\n' "$1" >> "$repo/packaging/systemd/fastwisperd.service"
}
run() { local script=$1; shift; [[ "$script" != install ]] || set -- --no-activate "$@"; bash "$repo/packaging/omarchy/$script.sh" "$@" > /dev/null; }
version A
run install
[[ $("$HOME/.local/bin/fastwisper") == A ]]
cp "$XDG_CONFIG_HOME/systemd/user/fastwisperd.service" "$scratch/unit-A"
cp "$XDG_CONFIG_HOME/omarchy/plugins/fastwisper.dictation/FastWisper.qml" "$scratch/qml-A"
cp "$XDG_DATA_HOME/fastwisper/hyprland.lua" "$scratch/lua-A"
version B
run install
[[ $("$HOME/.local/bin/fastwisperd") == B ]]
run rollback
[[ $("$HOME/.local/bin/fastwisper") == A ]]
cmp "$scratch/unit-A" "$XDG_CONFIG_HOME/systemd/user/fastwisperd.service"
cmp "$scratch/qml-A" "$XDG_CONFIG_HOME/omarchy/plugins/fastwisper.dictation/FastWisper.qml"
cmp "$scratch/lua-A" "$XDG_DATA_HOME/fastwisper/hyprland.lua"
run install
run install
[[ $(grep -cE 'BEGIN fastwisper' "$XDG_CONFIG_HOME/hypr/bindings.lua") == 1 ]]
# Arbitrary changes after install must survive upgrade, rollback and uninstall.
echo '-- new foreign bind F9' >> "$XDG_CONFIG_HOME/hypr/bindings.lua"
echo external > "$HOME/.local/bin/fastwisper"
echo external >> "$XDG_CONFIG_HOME/omarchy/plugins/fastwisper.dictation/FastWisper.qml"
echo external > "$XDG_CONFIG_HOME/systemd/user/fastwisperd.service"
echo '{"selected":"other.backend"}' > "$XDG_CONFIG_HOME/omarchy/shell.json"
version C
run install
[[ $(cat "$HOME/.local/bin/fastwisper") == external ]]
run rollback
run uninstall
run uninstall
[[ $(cat "$HOME/.local/bin/fastwisper") == external ]]
[[ $(cat "$XDG_CONFIG_HOME/systemd/user/fastwisperd.service") == external ]]
grep -qE 'new foreign bind' "$XDG_CONFIG_HOME/hypr/bindings.lua"
refute grep -qE 'BEGIN fastwisper' "$XDG_CONFIG_HOME/hypr/bindings.lua"
grep -qE 'external' "$XDG_CONFIG_HOME/omarchy/plugins/fastwisper.dictation/FastWisper.qml"
grep -qE 'other.backend' "$XDG_CONFIG_HOME/omarchy/shell.json"
[[ $(cat "$XDG_CONFIG_HOME/fastwisper/config.toml") == 'external config' ]]
[[ $(cat "$XDG_DATA_HOME/fastwisper/models/keep") == 'external model' ]]
# Preflight failure and subsequent retry.
rm "$FASTWISPER_BUILD_DIR/fastwisperd"
if run install; then echo 'missing candidate accepted' >&2; exit 1; fi
version D
# Failure after first managed write must unwind it.
rm "$HOME/.local/bin/fastwisper"
rm "$XDG_CONFIG_HOME/systemd/user/fastwisperd.service"
cat > "$scratch/mock/install" <<'MOCK'
#!/bin/bash
for argument in "$@"; do
  if [[ "$argument" == */packaging/systemd/fastwisperd.service ]]; then exit 42; fi
done
exec /usr/bin/install "$@"
MOCK
chmod +x "$scratch/mock/install"
if run install; then echo 'invalid destination accepted' >&2; exit 1; fi
[[ ! -e "$HOME/.local/bin/fastwisper" ]]
[[ ! -e "$HOME/.local/bin/fastwisperd" ]]
rm "$scratch/mock/install"
run install
run uninstall
# Abrupt interruption leaves a journal; next operation recovers it.
cat > "$scratch/mock/install" <<'MOCK'
#!/bin/bash
for argument in "$@"; do
  if [[ "$argument" == */packaging/systemd/fastwisperd.service ]]; then kill -KILL "$PPID"; exit 42; fi
done
exec /usr/bin/install "$@"
MOCK
chmod +x "$scratch/mock/install"
if run install; then echo 'kill injection failed' >&2; exit 1; fi
rm "$scratch/mock/install"
run uninstall
[[ ! -e "$HOME/.local/bin/fastwisper" ]]
[[ ! -e "$HOME/.local/bin/fastwisperd" ]]
# Concurrent operations must fail without touching the installation.
exec 8>"$XDG_STATE_HOME/fastwisper/packaging/lock"
flock 8
if run install; then echo 'lock ignored' >&2; exit 1; fi
flock -u 8
# A user-owned Alt+Space is kept byte for byte; the block is still added
# once (the hotkey comes from [hotkey]) and the conflict is reported.
echo 'hl.bind("ALT", "SPACE", "exec", "foreign")' >> "$XDG_CONFIG_HOME/hypr/bindings.lua"
cp "$XDG_CONFIG_HOME/hypr/bindings.lua" "$scratch/bindings-user"
run install 2> "$scratch/conflict"
grep -qF 'ALT+SPACE' "$scratch/conflict"
grep -qF 'hotkey.dictate' "$scratch/conflict"
[[ $(grep -cxF -- '-- BEGIN fastwisper' "$XDG_CONFIG_HOME/hypr/bindings.lua") == 1 ]]
grep -qxF 'hl.bind("ALT", "SPACE", "exec", "foreign")' "$XDG_CONFIG_HOME/hypr/bindings.lua"
run install 2> /dev/null
[[ $(grep -cxF -- '-- BEGIN fastwisper' "$XDG_CONFIG_HOME/hypr/bindings.lua") == 1 ]]
run uninstall
cmp "$scratch/bindings-user" "$XDG_CONFIG_HOME/hypr/bindings.lua"
# Migration from the pre-ledger installer is automatic and reversible.
printf '#!/bin/bash\necho legacy\n' > "$HOME/.local/bin/fastwisper"
chmod +x "$HOME/.local/bin/fastwisper"
if bash "$repo/packaging/omarchy/install.sh" --no-activate --adopt-existing > /dev/null; then
  echo 'unrecognized legacy adopted' >&2; exit 1
fi
printf '\n-- BEGIN fastwisper\ndofile("%s/fastwisper/hyprland.lua")\n-- END fastwisper\n' "$XDG_DATA_HOME" >> "$XDG_CONFIG_HOME/hypr/bindings.lua"
run install
[[ $("$HOME/.local/bin/fastwisper") == D ]]
run rollback
[[ $("$HOME/.local/bin/fastwisper") == legacy ]]
rm "$HOME/.local/bin/fastwisper"
# Reject binding ancestors before creating a generation or writing any payload.
mkdir "$scratch/foreign-hypr"
printf '%s\n' '-- foreign original' > "$scratch/foreign-hypr/bindings.lua"
chmod 0644 "$scratch/foreign-hypr/bindings.lua"
cp -p "$scratch/foreign-hypr/bindings.lua" "$scratch/foreign-bindings-before"
mv "$XDG_CONFIG_HOME/hypr" "$scratch/original-hypr"
ln -s "$scratch/foreign-hypr" "$XDG_CONFIG_HOME/hypr"
find "$XDG_STATE_HOME/fastwisper/packaging" -mindepth 1 -printf '%P\n' | sort > "$scratch/state-before"
for option in '' --adopt-existing; do
  if bash "$repo/packaging/omarchy/install.sh" $option > /dev/null; then
    echo 'binding ancestor symlink accepted' >&2; exit 1
  fi
  cmp "$scratch/foreign-bindings-before" "$scratch/foreign-hypr/bindings.lua"
  [[ $(stat -c '%a' "$scratch/foreign-hypr/bindings.lua") == 644 ]]
  [[ $(find "$scratch/foreign-hypr" -mindepth 1 | wc -l) == 1 ]]
  [[ ! -e "$HOME/.local/bin/fastwisper" && ! -e "$HOME/.local/bin/fastwisperd" ]]
  [[ ! -e "$XDG_DATA_HOME/fastwisper/hyprland.lua" ]]
  find "$XDG_STATE_HOME/fastwisper/packaging" -mindepth 1 -printf '%P\n' | sort > "$scratch/state-after"
  cmp "$scratch/state-before" "$scratch/state-after"
done
run uninstall
cmp "$scratch/foreign-bindings-before" "$scratch/foreign-hypr/bindings.lua"
rm "$XDG_CONFIG_HOME/hypr"
mv "$scratch/original-hypr" "$XDG_CONFIG_HOME/hypr"
# Refuse symlinked destination ancestors.
mkdir "$scratch/foreign"
rm -rf "$XDG_CONFIG_HOME/systemd"
ln -s "$scratch/foreign" "$XDG_CONFIG_HOME/systemd"
if run install; then echo 'symlink accepted' >&2; exit 1; fi
[[ ! -e "$scratch/foreign/user/fastwisperd.service" ]]
[[ ! -e "$CALL_LOG" ]]
echo 'packaging contract: PASS (A→B→A, external edits, idempotence, failures, symlinks, zero activation/network)'
# Rodado sozinho, o teste de ativação recusa em vez de passar vazio.
if (unset scratch; bash "$repo/packaging/omarchy/tests/activation.sh") 2> "$scratch/activation-alone"; then
  echo 'activation.sh sozinho passou sem testar nada' >&2; exit 1
fi
grep -q 'contract.sh' "$scratch/activation-alone"
source "$repo/packaging/omarchy/tests/activation.sh"
