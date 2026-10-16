#!/usr/bin/env bash
# Contrato da varredura genérica (leak-scan.sh sem --private): uma árvore
# sintética por categoria reprova com a categoria certa, sem imprimir o
# trecho; a árvore limpa e as exceções documentadas passam.
# Os segredos falsos são montados em tempo de execução (nada de token literal
# no repositório).
set -euo pipefail
scan=$(cd -- "$(dirname -- "$0")/.." && pwd)/leak-scan.sh
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
fail() { echo "leak-scan contract: $*" >&2; exit 1; }
n=0
# Montados aqui para o próprio teste não reprovar na varredura do repositório.
personal_email="jane.doe@mail-provider"".net"
home_a="/home""/jdoe" home_b="/home""/alice.smith"

# Árvore nova (em $t) com um README limpo.
new_tree() { n=$((n + 1)); t=$scratch/t$n; mkdir -p "$t"; echo "# Example" > "$t/README.md"; }
put() { mkdir -p "$(dirname -- "$1")"; printf '%b' "$2" > "$1"; } # put ARQUIVO CONTEÚDO
repeat() { local s=; for ((i = 0; i < $2; i++)); do s+=$1; done; printf '%s' "$s"; }

expect_clean() { # ÁRVORE DESCRIÇÃO
  local out
  out=$(bash "$scan" "$1" 2>&1) || fail "$2: expected clean, got: $out"
}
expect_finding() { # ÁRVORE "arquivo[:linha]: categoria" DESCRIÇÃO [TRECHO-QUE-NÃO-PODE-SAIR]
  local out status=0
  out=$(bash "$scan" "$1" 2>&1) || status=$?
  ((status == 1)) || fail "$3: expected exit 1, got $status: $out"
  grep -qxF -- "$2" <<< "$out" || fail "$3: missing '$2' in: $out"
  if [[ -n ${4:-} ]] && grep -qF -- "$4" <<< "$out"; then fail "$3: output leaks the matched text"; fi
  return 0
}

# PNG mínimo: assinatura, IHDR, chunks extras e IEND (CRC não é conferido).
png_chunk() { # TIPO DADOS-printf
  local data len
  data=$(printf '%b' "$2" | od -An -v -tx1 | tr -d ' \n')
  len=$((${#data} / 2))
  printf '\\x%02x\\x%02x\\x%02x\\x%02x%s%s\\x00\\x00\\x00\\x00' $((len >> 24 & 255)) $((len >> 16 & 255)) $((len >> 8 & 255)) $((len & 255)) "$1" "$2"
}
png() { # ARQUIVO CHUNKS-EXTRAS...
  local f=$1; shift
  put "$f" "\x89PNG\r\n\x1a\n$(png_chunk IHDR '\x00\x00\x00\x01\x00\x00\x00\x01\x08\x02\x00\x00\x00')$*$(png_chunk IEND '')"
}

# --- árvore limpa e exceções -------------------------------------------
new_tree
put "$t/src/main.rs" 'fn main() {\n    let home = "/home/user/.config";\n    let scratch = format!("{tmp}/home/config");\n}\n'
put "$t/src/lib/home/Stats.svelte" '<p>stats</p>\n'
put "$t/docs/contact.md" 'Write to someone@example.com or user@team.example.org, noreply@github.com.\nCI: /home/runner/work\n'
put "$t/scripts/test.sh" 'HOME="$scratch/home" XDG_DATA_HOME="$scratch/home/data"\ncp x /home/$user/\n'
put "$t/.env.example" 'API_KEY=\nOTHER=changeme\n'
put "$t/config/keys.md" 'Paste a token like ghp_XXXX here (never commit it).\n'
png "$t/docs/images/clean.png" "$(png_chunk IDAT 'tEXtnot-a-chunk-just-pixels')"
put "$t/docs/images/clean.jpg" '\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00\xff\xda\x00\x02Exif\xff\xd9'
put "$t/docs/images/clean.webp" 'RIFF\x16\x00\x00\x00WEBPVP8 \x04\x00\x00\x00\x00\x00\x00\x00'
mkdir -p "$t/tests"; head -c $((1024 * 1024)) /dev/zero > "$t/tests/fixture.bin"
expect_clean "$t" 'clean tree'

# --- segredos ----------------------------------------------------------
github_token="ghp_$(repeat a1B2 9)"
new_tree; put "$t/src/client.rs" "// token\nconst T: &str = \"$github_token\";\n"
expect_finding "$t" 'src/client.rs:2: secret' 'GitHub token' "$github_token"

openai_key="sk-proj-$(repeat Ab3_ 10)"
new_tree; put "$t/notes.txt" "key=$openai_key\n"
expect_finding "$t" 'notes.txt:1: secret' 'OpenAI key' "$openai_key"

anthropic_key="sk-ant-api03-$(repeat Zz9- 8)"
new_tree; put "$t/a/b.toml" "\n\napi_key = \"$anthropic_key\"\n"
expect_finding "$t" 'a/b.toml:3: secret' 'Anthropic key' "$anthropic_key"

aws_key="AKIA$(repeat Q7 8)"
new_tree; put "$t/deploy.sh" "export AWS_ACCESS_KEY_ID=$aws_key\n"
expect_finding "$t" 'deploy.sh:1: secret' 'AWS access key' "$aws_key"

aws_secret=$(repeat aB3/ 10)
new_tree; put "$t/creds" "[default]\nAWS_Secret_Access_Key = $aws_secret\n"
expect_finding "$t" 'creds:2: secret' 'AWS secret key' "$aws_secret"

pem="-----BEGIN OPENSSH $(printf PRIVATE) KEY-----"
new_tree; put "$t/id_test" "$pem\nAAAA\n-----END-----\n"
expect_finding "$t" 'id_test:1: secret' 'PEM private key'

# --- .env com valores --------------------------------------------------
new_tree; put "$t/ui/.env" '# local\nAPI_URL=\nexport TOKEN=abc123\n'
expect_finding "$t" 'ui/.env:3: env-file' '.env with a value' 'abc123'
new_tree; put "$t/prod.env" 'DB_PASSWORD = hunter2\n'
expect_finding "$t" 'prod.env:1: env-file' '*.env with a value' 'hunter2'

# --- /home/<nome> ------------------------------------------------------
new_tree; put "$t/src/paths.rs" "let a = 1;\nlet p = \"$home_a/.config/app\";\n"
expect_finding "$t" 'src/paths.rs:2: home-path' '/home/<name>' 'jdoe'
new_tree; put "$t/README.md" "Run \`cd $home_b/src\`.\n"
expect_finding "$t" 'README.md:1: home-path' '/home/<name> in Markdown' 'alice'

# --- e-mails -----------------------------------------------------------
new_tree; put "$t/LICENSE" "Copyright (c) Jane <$personal_email>\n"
expect_finding "$t" 'LICENSE:1: email' 'personal e-mail' 'jane.doe'

# --- binários e imagens ------------------------------------------------
new_tree; mkdir -p "$t/assets"; head -c $((1024 * 1024 + 1)) /dev/zero > "$t/assets/model.bin"
expect_finding "$t" 'assets/model.bin: large-binary' 'binary over 1 MiB'

new_tree; mkdir -p "$t/docs/images"
for i in 1 2 3; do head -c $((800 * 1024)) /dev/zero > "$t/docs/images/shot$i.png"; done
expect_finding "$t" 'docs/images/: images-total' 'docs/images/ over 2 MiB'

new_tree; png "$t/media/shot.png" "$(png_chunk tEXt 'Comment\x00made on a laptop')$(png_chunk IDAT 'x')"
expect_finding "$t" 'media/shot.png: image-metadata' 'PNG tEXt'
new_tree; png "$t/ui/static/icon.png" "$(png_chunk iTXt 'XML:com.adobe.xmp\x00\x00\x00\x00\x00<x/>')"
expect_finding "$t" 'ui/static/icon.png: image-metadata' 'PNG iTXt'
new_tree; png "$t/a.png" "$(png_chunk zTXt 'Raw\x00\x00x')"
expect_finding "$t" 'a.png: image-metadata' 'PNG zTXt'
new_tree; png "$t/b.png" "$(png_chunk eXIf 'MM\x00*')"
expect_finding "$t" 'b.png: image-metadata' 'PNG eXIf'

new_tree; put "$t/media/photo.jpg" '\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00\xff\xe1\x00\x10Exif\x00\x00MM\x00*\x00\x00\x00\x08\xff\xda\x00\x02\xff\xd9'
expect_finding "$t" 'media/photo.jpg: image-metadata' 'JPEG EXIF'
new_tree; put "$t/c.jpg" '\xff\xd8\xff\xfe\x00\x07hello\xff\xda\x00\x02\xff\xd9'
expect_finding "$t" 'c.jpg: image-metadata' 'JPEG comment'

new_tree; put "$t/d.webp" 'RIFF\x22\x00\x00\x00WEBPVP8 \x04\x00\x00\x00\x00\x00\x00\x00EXIF\x04\x00\x00\x00MM\x00*'
expect_finding "$t" 'd.webp: image-metadata' 'WebP EXIF'

# --- checkout git: vale o que o git publicaria -------------------------
if command -v git > /dev/null; then
  new_tree
  put "$t/.gitignore" 'local.log\n'
  put "$t/local.log" "contact: $personal_email\n"
  git -C "$t" init -q
  expect_clean "$t" 'ignored file in a git checkout'
  put "$t/new.txt" "contact: $personal_email\n"
  expect_finding "$t" 'new.txt:1: email' 'untracked file in a git checkout'
fi

# --- uso ---------------------------------------------------------------
status=0; bash "$scan" > /dev/null 2>&1 || status=$?
((status == 2)) || fail "no tree: expected exit 2, got $status"
status=0; bash "$scan" "$scratch/missing" > /dev/null 2>&1 || status=$?
((status == 2)) || fail "missing tree: expected exit 2, got $status"

echo "leak-scan contract: ok ($n trees)"
