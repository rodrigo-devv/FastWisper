#!/usr/bin/env bash
# Contrato dos recibos sem modelos, daemon, Ollama ou serviços reais.
set -euo pipefail
root=$(cd -- "$(dirname -- "$0")/../../.." && pwd)
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
bash "$root/scripts/network/isolated-egress.sh" --help >/dev/null
bash "$root/scripts/network/systemd-deny.sh" --help >/dev/null
egress=(bash "$root/scripts/network/isolated-egress.sh")
arguments=(--source-store "$scratch/store" --strace /bin/true --daemon /bin/true --cli /bin/true --test-binary /bin/true)
"${egress[@]}" --dry-run --output "$scratch/egress" "${arguments[@]}" >/dev/null
jq -e '.dry_run and .complete and (.aborted|not) and .gates=={} and .commands==[] and .blobs==[] and .artifact_sha256=={}' "$scratch/egress/receipt.json" >/dev/null
# Falha de jq na atualização não substitui o recibo inicial válido.
mkdir "$scratch/jq-failure-bin"
cat > "$scratch/jq-failure-bin/jq" <<'MOCK'
#!/usr/bin/env bash
for argument in "$@"; do
    [[ $argument != */receipt.json ]] || exit 42
done
exec /usr/bin/jq "$@"
MOCK
chmod +x "$scratch/jq-failure-bin/jq"
rc=0
PATH="$scratch/jq-failure-bin:$PATH" "${egress[@]}" --dry-run --output "$scratch/jq-failure" "${arguments[@]}" >/dev/null 2> "$scratch/error" || rc=$?
[[ $rc == 42 ]]
jq -e '.complete==false and .gates=={} and .commands==[]' "$scratch/jq-failure/receipt.json" >/dev/null
# Pin do LLM é verificação: divergente gera texto (isolated_real_ollama, com
# aviso); só ausente exige o fallback para as regras.
for probe in correct:isolated_real_ollama:real_inference divergent:isolated_real_ollama:real_pin_warning missing:isolated_unavailable_ollama:real_pin_fallback; do
    IFS=: read -r scenario test gate <<< "$probe"
    "${egress[@]}" --dry-run --scenario "$scenario" --output "$scratch/probe-$scenario" "${arguments[@]}" >/dev/null
    jq -e --arg scenario "$scenario" --arg test "$test" --arg gate "$gate" '.scenario==$scenario and .probe=={test:$test,gate:$gate} and .gates=={}' "$scratch/probe-$scenario/receipt.json" >/dev/null
done
# Dry-run valida presença dos argumentos, sem exigir executáveis/modelos reais.
for option in source-store strace daemon cli test-binary; do
    filtered=()
    for ((i=0;i<${#arguments[@]};i+=2)); do
        [[ ${arguments[i]} == "--$option" ]] || filtered+=("${arguments[i]}" "${arguments[i+1]}")
    done
    rc=0
    "${egress[@]}" --dry-run --output "$scratch/missing-$option" "${filtered[@]}" > /dev/null 2> "$scratch/error" || rc=$?
    [[ $rc == 2 ]]
    grep -q -- "Argumento obrigatório ausente: --$option" "$scratch/error"
done
rc=0
"${egress[@]}" --output "$scratch/aborted" "${arguments[@]}" --strace /nonexistent > /dev/null 2> "$scratch/error" || rc=$?
[[ $rc == 2 ]]
jq -e '.complete==false and .aborted and .returncode==2 and .ollama_version==null and .traces.ollama.files==0 and (.limits|length)>0' "$scratch/aborted/receipt.json" >/dev/null
# Falha posterior preserva traces parciais e diagnósticos, sem processos reais.
mkdir -p "$scratch/store/manifests/registry.ollama.ai/library/qwen3"
for failure in manifest blob size hash; do
    output="$scratch/$failure"
    mkdir "$output"
    printf 'connect(1, {sa_family=AF_INET, sin_addr=inet_addr("192.0.2.2")}, 16) = -1\n' > "$output/ollama.trace.1"
    # O diretório de saída deve ser novo; o mock de cmp injeta o trace após mkdir.
    mv "$output" "$scratch/seed"
    mkdir -p "$scratch/diagnostic-bin"
    cat > "$scratch/diagnostic-bin/cmp" <<'MOCK'
#!/usr/bin/env bash
/bin/cp "$SEED/ollama.trace.1" "$OUTPUT/ollama.trace.1"
[[ $FAILURE != manifest ]]
MOCK
    cat > "$scratch/diagnostic-bin/stat" <<'MOCK'
#!/usr/bin/env bash
[[ $FAILURE != size ]] || { echo 0; exit; }
echo 2497280480
MOCK
    chmod +x "$scratch/diagnostic-bin/"*
    name=$(jq -r '.layers[0].digest|sub(":";"-")' "$root/crates/fw-core/data/qwen3-4b-instruct.manifest.json")
    mkdir -p "$scratch/store/blobs"
    [[ $failure != hash && $failure != size ]] || touch "$scratch/store/blobs/$name"
    rc=0
    PATH="$scratch/diagnostic-bin:$PATH" SEED="$scratch/seed" OUTPUT="$output" FAILURE="$failure" "${egress[@]}" --output "$output" "${arguments[@]}" >/dev/null 2> "$scratch/error" || rc=$?
    [[ $rc == 1 ]]
    case $failure in
        manifest) grep -q 'Manifesto difere dos bytes fixados' "$scratch/error" ;;
        blob|size) grep -q 'Blob ausente ou com tamanho errado' "$scratch/error" ;;
        hash) grep -q 'Hash do blob copiado diverge' "$scratch/error" ;;
    esac
    jq -e '.complete==false and .aborted and .returncode==1 and .traces.ollama.non_loopback_addresses==["192.0.2.2"]' "$output/receipt.json" >/dev/null
    rm -rf "$scratch/seed" "$scratch/store/blobs"
done
bash "$root/scripts/network/systemd-deny.sh" --dry-run --output "$scratch/deny.json"
jq -e '.verdict=="unavailable" and .dry_run and .rows=={}' "$scratch/deny.json" >/dev/null
# Unidades simuladas; sinks socat são locais e temporários.
mkdir "$scratch/bin"
cat > "$scratch/bin/systemd-run" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
if [[ $* != *IPAddressDeny=any* ]]; then
    [[ $VERDICT != unavailable ]] || exit 1
    echo '[{"host":"127.0.0.1","connected":true},{"host":"192.0.2.1","connected":true}]'
else
    case $VERDICT in
        enforced) echo '[{"host":"127.0.0.1","connected":true},{"host":"192.0.2.1","connected":false}]' ;;
        inconclusive) echo '[{"host":"127.0.0.1","connected":false},{"host":"192.0.2.1","connected":false}]' ;;
        *) echo '[{"host":"127.0.0.1","connected":true},{"host":"192.0.2.1","connected":true}]' ;;
    esac
    [[ $VERDICT != unsupported ]] || echo 'lacks privileges' >&2
fi
MOCK
cat > "$scratch/bin/ip" <<'MOCK'
#!/usr/bin/env bash
printf '[{"addr_info":[{"scope":"global","local":"127.0.0.2"}]}]\n'
MOCK
cat > "$scratch/bin/journalctl" <<'MOCK'
#!/usr/bin/env bash
exit 0
MOCK
cat > "$scratch/bin/systemctl" <<'MOCK'
#!/usr/bin/env bash
echo 'systemd (simulado)'
MOCK
chmod +x "$scratch/bin/"*
for verdict in enforced not_enforced unsupported unavailable inconclusive; do
    PATH="$scratch/bin:$PATH" VERDICT="$verdict" bash "$root/scripts/network/systemd-deny.sh" --output "$scratch/$verdict.json" >/dev/null
    jq -e --arg verdict "$verdict" '.verdict==$verdict and (.rows.control.command|type)=="array"' "$scratch/$verdict.json" >/dev/null
done
mkdir "$scratch/traces"
cat > "$scratch/traces/daemon.trace.1" <<'TRACE'
connect(1, {sa_family=AF_INET, sin_addr=inet_addr("127.0.0.1")}, 16) = 0
connect(1, {sa_family=AF_INET, sin_addr=inet_addr("192.0.2.2")}, 16) = -1
connect(1, {sa_family=AF_INET6, inet_pton(AF_INET6, "::1", &sin6_addr)}, 28) = 0
connect(1, {sa_family=AF_INET6, inet_pton(AF_INET6, "::ffff:127.0.0.1", &sin6_addr)}, 28) = 0
connect(1, {sa_family=AF_INET6, inet_pton(AF_INET6, "0:0::1", &sin6_addr)}, 28) = 0
connect(1, {sa_family=AF_INET6, inet_pton(AF_INET6, "::ffff:7f00:1", &sin6_addr)}, 28) = 0
connect(1, {sa_family=AF_INET6, inet_pton(AF_INET6, "2001:db8::2", &sin6_addr)}, 28) = -1
TRACE
node "$root/scripts/network/trace-summary.cjs" "$scratch/traces" > "$scratch/traces.json"
jq -e '.daemon.files==1 and .daemon.non_loopback_addresses==["192.0.2.2","2001:db8::2"] and (.daemon.outbound_ip_events|length)==7 and .ollama.files==0' "$scratch/traces.json" >/dev/null
echo 'Contratos dos coletores: passaram'
