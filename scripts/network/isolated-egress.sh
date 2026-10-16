#!/usr/bin/env bash
# Processos próprios, store verificado e PipeWire privado; nunca usa produção.
set -euo pipefail
root=$(cd -- "$(dirname -- "$0")/../.." && pwd)
manifest="$root/crates/fw-core/data/qwen3-4b-instruct.manifest.json"
source_store='' output='' strace='' daemon='' cli='' test_binary=''
local_only=false dry_run=false scenario=correct idle_seconds=10
while (($#)); do
    case $1 in
        --help) echo 'Uso: isolated-egress.sh --output diretório-novo [--dry-run] --strace bin --daemon bin --cli bin --test-binary bin [--source-store store (obrigatório em correct/divergent)] [--local-only] [--scenario correct|missing|divergent] [--idle-seconds N]'; exit ;;
        --source-store) source_store=${2:?}; shift ;;
        --output) output=${2:?}; shift ;;
        --strace) strace=${2:?}; shift ;;
        --daemon) daemon=${2:?}; shift ;;
        --cli) cli=${2:?}; shift ;;
        --test-binary) test_binary=${2:?}; shift ;;
        --local-only) local_only=true ;;
        --dry-run) dry_run=true ;;
        --scenario) scenario=${2:?}; shift ;;
        --idle-seconds) idle_seconds=${2:?}; shift ;;
        *) echo "Opção desconhecida: $1" >&2; exit 2 ;;
    esac
    shift
done
[[ $scenario =~ ^(correct|missing|divergent)$ && $idle_seconds =~ ^[0-9]+$ && -n $output ]] || { echo 'Argumentos inválidos; consulte --help' >&2; exit 2; }
for option in strace daemon cli test_binary; do
    [[ -n ${!option} ]] || { echo "Argumento obrigatório ausente: --${option//_/-}" >&2; exit 2; }
done
# Pin do LLM é verificação, não proibição: digest divergente gera texto (com
# aviso); só o modelo ausente cai nas regras.
case $scenario in
    correct) test_name=isolated_real_ollama gate=real_inference ;;
    divergent) test_name=isolated_real_ollama gate=real_pin_warning ;;
    missing) test_name=isolated_unavailable_ollama gate=real_pin_fallback ;;
esac
if [[ $scenario != missing && -z $source_store ]]; then
    echo 'Argumento obrigatório ausente: --source-store (cenário correct ou divergent)' >&2
    exit 2
fi
mkdir -- "$output"
output=$(realpath -- "$output")
sha() { sha256sum -- "$1" | cut -d' ' -f1; }
receipt="$output/receipt.json"
jq -n --arg date "$(date -u +%FT%TZ)" --arg commit "$(git -C "$root" rev-parse HEAD)" --arg scenario "$scenario" --arg test "$test_name" --arg gate "$gate" --argjson local "$local_only" --arg digest "$(sha "$manifest")" '{recorded_at_utc:$date,source_commit:$commit,artifact_sha256:{},scenario:$scenario,probe:{test:$test,gate:$gate},local_only:$local,manifest_digest:$digest,blobs:[],commands:[],gates:{},method:"strace -ff -yy trace=network, payload strings suppressed; bounded window, not packet capture",traces:{},limits:[],complete:false,aborted:false}' > "$receipt"
update() { jq "$@" "$receipt" > "$output/receipt.tmp" && mv -- "$output/receipt.tmp" "$receipt"; }
abort_receipt() {
    local rc=${1:-$?}
    trap - EXIT
    set +e
    update --argjson rc "$rc" '.complete=false | .aborted=true | .returncode=$rc | .limits += ["Execução abortada: evidência parcial; não comprova ausência de egress"]'
    if node "$root/scripts/network/trace-summary.cjs" "$output" > "$output/traces.tmp"; then
        update --slurpfile traces "$output/traces.tmp" '.traces=$traces[0]'
    fi
    rm -f -- "$output/traces.tmp"
    exit "$rc"
}
trap abort_receipt EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
if $dry_run; then
    update '. + {dry_run:true,complete:true,elapsed_seconds:0,limits:["dry-run: nenhum processo iniciado; gates não exercitados"],ollama_version:null,ollama_default_overrides:[]}'
    trap - EXIT
    cat "$receipt"
    exit
fi
for binary in "$strace" "$daemon" "$cli" "$test_binary"; do [[ -x $binary ]] || { echo "Executável ausente: $binary" >&2; exit 2; }; done
strace=$(realpath -- "$strace") daemon=$(realpath -- "$daemon") cli=$(realpath -- "$cli") test_binary=$(realpath -- "$test_binary")
update --arg daemon "$(sha "$daemon")" --arg cli "$(sha "$cli")" --arg test "$(sha "$test_binary")" --arg strace "$(sha "$strace")" '.artifact_sha256={daemon:$daemon,cli:$cli,test_binary:$test,strace:$strace}'
scratch=$(mktemp -d) pids=() started=$SECONDS
stop_processes() {
    for pid in "${pids[@]}"; do kill -- "-$pid" 2>/dev/null || true; done
    for pid in "${pids[@]}"; do
        for ((i=0;i<100;i++)); do kill -0 "$pid" 2>/dev/null || break; sleep .1; done
        kill -KILL -- "-$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    done
    pids=()
}
cleanup() {
    local rc=$?
    set +e
    stop_processes
    rm -rf -- "$scratch"
    if [[ $(jq -r .complete "$receipt") != true ]]; then
        abort_receipt "$rc"
    fi
    exit "$rc"
}
trap cleanup EXIT
store="$scratch/models"
mkdir -p "$store/blobs" "$store/manifests/registry.ollama.ai/library/qwen3"
if [[ $scenario != missing ]]; then
    cmp -s -- "$manifest" "$source_store/manifests/registry.ollama.ai/library/qwen3/4b-instruct" || { echo 'Manifesto difere dos bytes fixados; nenhum pull permitido' >&2; exit 1; }
    while IFS= read -r blob; do
        digest=$(jq -r .digest <<< "$blob") size=$(jq -r .size <<< "$blob")
        name=${digest/:/-}
        [[ -f $source_store/blobs/$name && $(stat -c %s -- "$source_store/blobs/$name") == "$size" ]] || { echo "Blob ausente ou com tamanho errado: $name" >&2; exit 1; }
        cp --reflink=auto -- "$source_store/blobs/$name" "$store/blobs/$name"
        [[ $(sha "$store/blobs/$name") == "${digest#sha256:}" ]] || { echo "Hash do blob copiado diverge: $name" >&2; exit 1; }
        update --argjson blob "$blob" '.blobs += [$blob + {verified:true} | {digest,size,verified}]'
    done < <(jq -c '.layers[] , .config' "$manifest")
    cp -- "$manifest" "$store/manifests/registry.ollama.ai/library/qwen3/4b-instruct"
    [[ $scenario != divergent ]] || printf '\n' >> "$store/manifests/registry.ollama.ai/library/qwen3/4b-instruct"
fi
for name in home runtime config data cache mock-bin; do mkdir -m700 -- "$scratch/$name"; done
for command in hyprctl wl-copy wl-paste notify-send systemctl; do printf '#!/bin/sh\nexit 1\n' > "$scratch/mock-bin/$command"; chmod 700 "$scratch/mock-bin/$command"; done
process_env=(env -i "PATH=$scratch/mock-bin:/usr/bin:/bin" "HOME=$scratch/home" "XDG_RUNTIME_DIR=$scratch/runtime" "XDG_CONFIG_HOME=$scratch/config" "XDG_DATA_HOME=$scratch/data" "XDG_CACHE_HOME=$scratch/cache" LANG=C.UTF-8)
# Reserva efêmera sem privilégios; há corrida entre reserva e início, registrada como falha.
port=$(node -e 'const s=require("node:net").createServer();s.listen(0,"127.0.0.1",()=>{console.log(s.address().port);s.close()})')
endpoint="http://127.0.0.1:$port"
ollama_env=("${process_env[@]}" "OLLAMA_HOST=127.0.0.1:$port" "OLLAMA_MODELS=$store")
$local_only && ollama_env+=(OLLAMA_NO_CLOUD=1)
update --arg version "$("${ollama_env[@]}" /usr/bin/ollama --version)" '.ollama_version=$version | .ollama_default_overrides=["HOME/XDG (disposable)","OLLAMA_HOST (own loopback port)","OLLAMA_MODELS (verified scratch copy)"] | if .local_only then .ollama_default_overrides += ["OLLAMA_NO_CLOUD=1 (explicit comparison)"] else . end'
launch() {
    local label=$1 trace=$2; shift 2
    local command=("$@")
    if [[ $trace == true ]]; then command=("$strace" -ff -tt -yy -s 0 -e trace=network -o "$output/$label.trace" -- "${command[@]}"); fi
    local argv
    argv=$(printf '%s\n' "${command[@]}" | jq -R . | jq -s .)
    update --arg label "$label" --argjson argv "$argv" '.commands += [{label:$label,argv:$argv}]'
    setsid "${command[@]}" > "$output/$label.log" 2>&1 &
    launched=$!; pids+=("$launched")
}
ready() {
    local pid=$1; shift
    for ((i=0;i<150;i++)); do
        kill -0 "$pid" 2>/dev/null || { echo 'Processo isolado terminou antes de ficar pronto' >&2; return 1; }
        if "$@"; then return; fi
        sleep .1
    done
    echo 'Timeout de prontidão' >&2; return 1
}
tags_ready() {
    local tags
    tags=$(curl --silent --fail --noproxy '*' --max-time 1 "$endpoint/api/tags") || return 1
    jq -e --arg scenario "$scenario" --arg digest "$(sha "$manifest")" 'if $scenario=="missing" then (.models|length)==0 else any(.models[]; .name=="qwen3:4b-instruct" and (if $scenario=="correct" then .digest==$digest else .digest!=$digest end)) end' <<< "$tags" >/dev/null
}
launch ollama true "${ollama_env[@]}" /usr/bin/ollama serve
ollama_pid=$launched
ready "$ollama_pid" tags_ready
cat > "$scratch/pipewire.conf" <<'PW'
context.properties = { core.daemon = true core.name = pipewire-0 }
context.spa-libs = { support.* = support/libspa-support }
context.modules = [
 { name = libpipewire-module-protocol-native }
 { name = libpipewire-module-client-node }
 { name = libpipewire-module-metadata }
 { name = libpipewire-module-access }
]
PW
launch pipewire false "${process_env[@]}" /usr/bin/pipewire -c "$scratch/pipewire.conf"
ready "$launched" test -S "$scratch/runtime/pipewire-0"
printf '[audio]\ndevice = "fw-unavailable-test"\n[llm]\nendpoint = "%s"\n' "$endpoint" > "$scratch/fastwisper.toml"
launch daemon true "${process_env[@]}" "$daemon" --config "$scratch/fastwisper.toml"
daemon_pid=$launched
ready "$daemon_pid" test -S "$scratch/runtime/fastwisper.sock"
launch setup true timeout 15 "${process_env[@]}" "$cli" setup --config "$scratch/fastwisper.toml"
rc=0; wait "$launched" || rc=$?
created=false; [[ ! -d $scratch/data/fastwisper/models ]] || created=true
update --argjson rc "$rc" --argjson created "$created" '.gates.setup_no_download={returncode:$rc,model_directory_created:$created}'
rc=0; timeout 10 "${process_env[@]}" "$cli" status >"$scratch/status.out" 2>"$scratch/status.err" || rc=$?
update --argjson rc "$rc" --rawfile out "$scratch/status.out" --rawfile err "$scratch/status.err" '.gates.daemon_status={returncode:$rc,stdout:$out,stderr:$err}'
launch llm-probe true timeout 120 "${process_env[@]}" "FASTWISPER_ISOLATED_OLLAMA_ENDPOINT=$endpoint" "$test_binary" --exact "$test_name" --ignored --nocapture
result=0; wait "$launched" || result=$?
update --arg gate "$gate" --argjson rc "$result" '.gates[$gate]={returncode:$rc}'
sleep "$idle_seconds"
for label in daemon ollama; do
    pid=$daemon_pid; [[ $label != ollama ]] || pid=$ollama_pid
    alive=false; kill -0 "$pid" 2>/dev/null && alive=true
    update --arg label "${label}_alive" --argjson alive "$alive" '.gates[$label]=$alive'
done
stop_processes
update --argjson elapsed "$((SECONDS-started))" '.elapsed_seconds=$elapsed | .limits=["No physical microphone, ASR inference or paste; ASR missing in disposable HOME","Daemon startup/LLM warmup/IPC and synthetic real LLM inference observed","No systemd containment assumed; default Ollama cloud/proxy variables unset","Observation does not prove future traffic absence or production-service privacy"]'
# Classificação de endereços/eventos sem interpretar strings de payload.
node "$root/scripts/network/trace-summary.cjs" "$output" > "$scratch/traces.json"
update --slurpfile traces "$scratch/traces.json" '.traces=$traces[0]'
[[ $result == 0 ]]
update '.complete=true'
jq .traces "$receipt"
