#!/usr/bin/env bash
# Probe descartável: somente sinks locais e unidades transitórias do usuário.
set -euo pipefail
output='' dry_run=false
if [[ ${1:-} == --probe ]]; then
    jq -c '.[]' <<< "$2" | while IFS= read -r target; do
        host=$(jq -r '.[0]' <<< "$target")
        port=$(jq -r '.[1]' <<< "$target")
        if timeout 2 bash -c 'exec 3<>/dev/tcp/"$1"/"$2"' _ "$host" "$port" 2>/dev/null; then
            jq -nc --arg host "$host" '{host:$host,connected:true}'
        else
            jq -nc --arg host "$host" '{host:$host,connected:false,error:"conexão recusada, bloqueada ou timeout"}'
        fi
    done | jq -cs .
    exit
fi
while (($#)); do
    case $1 in
        --help) echo 'Uso: systemd-deny.sh --output arquivo.json [--dry-run]'; exit ;;
        --output) output=${2:?}; shift ;;
        --dry-run) dry_run=true ;;
        *) echo "Opção desconhecida: $1" >&2; exit 2 ;;
    esac
    shift
done
[[ -n $output ]] || { echo '--output obrigatório' >&2; exit 2; }
mkdir -p -- "$(dirname -- "$output")"
if $dry_run; then
    jq -n '{verdict:"unavailable",reason:"dry-run: nenhuma unidade ou conexão iniciada",method:"TCP to two local sinks; non-loopback IPv4 blocked only in deny unit",systemd:null,rows:{},dry_run:true}' > "$output"
    exit
fi
host=$(ip -j -4 address show | jq -r '[.[].addr_info[] | select(.scope=="global") | .local][0] // empty')
if [[ -z $host ]]; then
    jq -n '{verdict:"unavailable",reason:"no non-loopback local IPv4"}' > "$output"
    exit
fi
scratch=$(mktemp -d) pids=()
cleanup() { for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; done; rm -rf -- "$scratch"; }
trap cleanup EXIT
# socat escolhe portas efêmeras e informa getsockname no log.
targets='[]'
for address in 127.0.0.1 "$host"; do
    log="$scratch/sink-${#pids[@]}.log"
    : > "$log"
    socat -d -d "TCP4-LISTEN:0,bind=$address,reuseaddr,fork" OPEN:/dev/null >"$log" 2>&1 &
    pids+=("$!")
    port=''
    for ((i=0;i<100;i++)); do
        port=$(sed -nE 's/.*listening on AF=2 [^:]+:([0-9]+).*/\1/p' "$log")
        [[ -n $port ]] && break
        sleep .05
    done
    [[ -n $port ]] || { cat "$log" >&2; exit 1; }
    targets=$(jq -c --arg host "$address" --argjson port "$port" '. + [[$host,$port]]' <<< "$targets")
done
rows='{}' since=$(date '+%Y-%m-%d %H:%M:%S')
for mode in control deny; do
    unit="fastwisper-m6-probe-$(cat /proc/sys/kernel/random/uuid)"
    command=(systemd-run --user --wait --pipe --collect --unit "$unit")
    [[ $mode != deny ]] || command+=(-p IPAddressDeny=any -p IPAddressAllow=localhost)
    command+=(bash "$(realpath -- "$0")" --probe "$targets")
    rc=0
    timeout 20 "${command[@]}" >"$scratch/stdout" 2>"$scratch/stderr" || rc=$?
    timeout 10 journalctl --user -u "$unit" --since "$since" --no-pager -o cat >"$scratch/journal" 2>/dev/null || true
    argv=$(printf '%s\n' "${command[@]}" | jq -R . | jq -s .)
    observations=$(jq -Rsc 'split("\n") | map(fromjson? | select(type=="array")) | last // null' "$scratch/stdout")
    rows=$(jq --arg mode "$mode" --argjson command "$argv" --argjson rc "$rc" --rawfile stdout "$scratch/stdout" --rawfile stderr "$scratch/stderr" --rawfile journal "$scratch/journal" --argjson observations "$observations" '.[$mode]={command:$command,returncode:$rc,stdout:$stdout,stderr:$stderr,journal:$journal,observations:$observations}' <<< "$rows")
done
jq -n --argjson rows "$rows" --arg systemd "$(systemctl --version | head -n1)" '
{method:"TCP to two local sinks; non-loopback IPv4 blocked only in deny unit",systemd:$systemd,rows:$rows,verdict:
(if ($rows.control.observations == null or $rows.deny.observations == null or ($rows.control.observations | length) != 2 or ($rows.deny.observations | length) != 2 or ($rows.control.observations | all(.connected) | not)) then "unavailable"
elif $rows.deny.observations[0].connected and ($rows.deny.observations[1].connected | not) then "enforced"
elif $rows.deny.observations[1].connected and (($rows.deny.stderr + $rows.deny.journal) | ascii_downcase | test("not supported|not available|without bpf|bpf-based firewalling|lacks privileges")) then "unsupported"
elif $rows.deny.observations[1].connected then "not_enforced" else "inconclusive" end)}' > "$output"
jq -r .verdict "$output"
