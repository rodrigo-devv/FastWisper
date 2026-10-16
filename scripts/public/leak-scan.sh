#!/usr/bin/env bash
# Varredura de vazamento de uma árvore antes de publicar (ou em cada PR).
# Uso: scripts/public/leak-scan.sh [--private] TREE
#
# Checagens genéricas: segredos (tokens e chaves privadas), .env com valores,
# caminhos /home/<nome>, e-mails fora da lista de exemplo, binários grandes,
# total de docs/images/ e metadados em imagens (PNG, JPEG, WebP).
# --private (só no repositório de desenvolvimento): também os padrões de
# leak-patterns.private, arquivos fora da allowlist e links para docs/ que não
# existem na árvore.
#
# Saída: "arquivo:linha: categoria" ou "arquivo: categoria". O texto
# encontrado nunca é impresso. Exit 0 sem achados, 1 com achados, 2 uso.
# Ferramentas: bash, GNU grep, find, od, stat, sort (ubuntu-latest e Arch).
set -euo pipefail
export LC_ALL=C

usage() { echo 'usage: leak-scan.sh [--private] TREE' >&2; exit 2; }
here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
private=0 tree=
patterns=${LEAK_SCAN_PATTERNS:-$here/leak-patterns.private}
allowlist=${LEAK_SCAN_ALLOWLIST:-$here/allowlist}
while (($#)); do
  case $1 in
    --private) private=1; shift ;;
    -h|--help) usage ;;
    -*) usage ;;
    *) [[ -z $tree ]] || usage; tree=$1; shift ;;
  esac
done
[[ -n $tree && -d $tree ]] || usage
tree=$(cd -- "$tree" && pwd -P)

# Limites: 1 MiB por binário, 2 MiB para todas as imagens do README.
max_binary=$((1024 * 1024))
max_images=$((2 * 1024 * 1024))
# Únicos /home/<nome> aceitos: o exemplo da documentação e o runner do GitHub.
home_ok=' user runner '
# E-mails aceitos: domínios reservados para exemplo (RFC 2606/6761), os
# endereços noreply de commit e os domínios fictícios dos testes de snippets.
email_ok_domains=' example.com example.org example.net exemplo.com empresa.com x.com api.x.com users.noreply.github.com '
email_ok_tlds=' example test invalid localhost '
email_ok_addresses=' noreply@anthropic.com noreply@github.com git@github.com '

findings=()
found() { findings+=("$1: $2"); }

# Arquivos: num checkout git, o que o git publicaria (rastreado ou novo não
# ignorado); fora dele, tudo menos .git/.
files=()
if [[ $(git -C "$tree" rev-parse --show-toplevel 2>/dev/null) == "$tree" ]]; then
  while IFS= read -r -d '' f; do [[ -f $tree/$f && ! -L $tree/$f ]] && files+=("$f"); done \
    < <(git -C "$tree" ls-files -z --cached --others --exclude-standard)
else
  while IFS= read -r -d '' f; do files+=("${f#./}"); done \
    < <(cd "$tree" && find . -path ./.git -prune -o -type f -print0)
fi
((${#files[@]})) || { echo "leak-scan: no files in $tree" >&2; exit 2; }

# Texto = não vazio e sem bytes nulos (critério do grep -I).
text=() binary=()
declare -A is_text=()
while IFS= read -r -d '' f; do is_text[$f]=1; done \
  < <(cd "$tree" && printf '%s\0' "${files[@]}" | xargs -0 -r grep -lIZ -- . 2>/dev/null || true)
for f in "${files[@]}"; do
  if [[ -n ${is_text[$f]:-} ]]; then text+=("$f"); elif [[ -s $tree/$f ]]; then binary+=("$f"); fi
done

# grep -o nos arquivos de texto; imprime "arquivo:linha:trecho" para o filtro.
grep_text() {
  ((${#text[@]})) || return 0
  (cd "$tree" && printf '%s\0' "${text[@]}" | xargs -0 -r grep -HnoIE "$@" -- 2>/dev/null) || true
}
# Achado por linha, sem o trecho.
report_lines() { # CATEGORIA < saída de grep_text
  local file line rest
  while IFS=: read -r file line rest; do found "$file:$line" "$1"; done
}

# --- segredos -----------------------------------------------------------
report_lines secret < <({
  grep_text -e 'gh[pousr]_[A-Za-z0-9]{36}' -e 'github_pat_[A-Za-z0-9_]{50,}' \
    -e 'sk-ant-[A-Za-z0-9_-]{20,}' -e 'sk-(proj-|svcacct-|admin-)?[A-Za-z0-9_-]{32,}' \
    -e '(AKIA|ASIA)[0-9A-Z]{16}' -e 'xox[abposr]-[A-Za-z0-9-]{10,}' -e 'AIza[0-9A-Za-z_-]{35}' \
    -e 'glpat-[A-Za-z0-9_-]{20,}' -e 'hf_[A-Za-z0-9]{34,}' -e 'npm_[A-Za-z0-9]{36}' \
    -e '-----BEGIN[ A-Z0-9]*PRIVATE KEY( BLOCK)?-----'
  grep_text -i -e 'aws_secret_access_key[[:space:]]*[=:][[:space:]]*["'\'']?[A-Za-z0-9/+=]{40}'
} | sort -u)

# --- .env com valores ---------------------------------------------------
for f in "${text[@]}"; do
  base=${f##*/}
  [[ $base == .env || $base == .env.* || $base == *.env ]] || continue
  [[ $base =~ \.(example|sample|template)$ ]] && continue
  while IFS=: read -r line _; do found "$f:$line" env-file; done \
    < <(grep -nE '^[[:space:]]*(export[[:space:]]+)?[A-Za-z_][A-Za-z0-9_]*[[:space:]]*=[[:space:]]*[^[:space:]#]' -- "$tree/$f" || true)
done

# --- /home/<nome> -------------------------------------------------------
# Só caminho absoluto: "$tmp/home/x" ou "lib/home/X.svelte" não contam.
while IFS=: read -r file line match; do
  name=${match#*/home/}
  name=${name%"${name##*[!.]}"} # ponto final da frase não faz parte do nome
  [[ $home_ok == *" $name "* ]] || found "$file:$line" home-path
done < <(grep_text -e '(^|[^A-Za-z0-9_.$}/-])/home/[A-Za-z0-9_][A-Za-z0-9_.-]*' | sort -u)

# --- e-mails ------------------------------------------------------------
while IFS=: read -r file line match; do
  address=${match,,} domain=${match#*@}
  domain=${domain,,}
  [[ $email_ok_addresses == *" $address "* ]] && continue
  [[ $email_ok_tlds == *" ${domain##*.} "* ]] && continue
  ok=0 d=$domain
  while [[ $d == *.* ]]; do # o domínio ou um pai dele (team.example.org)
    [[ $email_ok_domains == *" $d "* ]] && { ok=1; break; }
    d=${d#*.}
  done
  ((ok)) || found "$file:$line" email
done < <(grep_text -e '[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}' | sort -u)

# --- binários grandes e imagens -----------------------------------------
bytes() { od -An -v -tu1 -j "$2" -N "$3" -- "$1"; } # ARQUIVO OFFSET N
fourcc() { printf "$(printf '\\%03o' "$@")"; }
png_metadata() { # 0 se houver chunk de texto/EXIF
  local f=$1 size off=8 b len type
  size=$(stat -c %s -- "$f")
  while ((off + 8 <= size)); do
    read -r -a b < <(bytes "$f" "$off" 8)
    ((${#b[@]} == 8)) || return 1
    len=$(((b[0] << 24) | (b[1] << 16) | (b[2] << 8) | b[3]))
    type=$(fourcc "${b[@]:4:4}")
    case $type in tEXt|iTXt|zTXt|eXIf) return 0 ;; IEND) return 1 ;; esac
    off=$((off + 12 + len))
  done
  return 1
}
jpeg_metadata() { # APP1 (EXIF/XMP), APP13 (IPTC) ou comentário
  local f=$1 size off=2 b marker len
  size=$(stat -c %s -- "$f")
  while ((off + 4 <= size)); do
    read -r -a b < <(bytes "$f" "$off" 4)
    ((b[0] == 255)) || return 1
    marker=${b[1]}
    if ((marker == 255)); then off=$((off + 1)); continue; fi
    if ((marker == 1 || (marker >= 208 && marker <= 216))); then off=$((off + 2)); continue; fi
    ((marker == 218 || marker == 217)) && return 1 # SOS/EOI: fim dos cabeçalhos
    ((marker == 225 || marker == 237 || marker == 254)) && return 0
    len=$(((b[2] << 8) | b[3]))
    off=$((off + 2 + len))
  done
  return 1
}
webp_metadata() { # chunks EXIF ou XMP
  local f=$1 size off=12 b len type
  size=$(stat -c %s -- "$f")
  while ((off + 8 <= size)); do
    read -r -a b < <(bytes "$f" "$off" 8)
    ((${#b[@]} == 8)) || return 1
    type=$(fourcc "${b[@]:0:4}")
    len=$((b[4] | (b[5] << 8) | (b[6] << 16) | (b[7] << 24)))
    [[ $type == EXIF || $type == 'XMP ' ]] && return 0
    off=$((off + 8 + len + (len & 1)))
  done
  return 1
}
images_total=0
for f in "${files[@]}"; do
  [[ $f == docs/images/* ]] && images_total=$((images_total + $(stat -c %s -- "$tree/$f")))
done
for f in "${binary[@]}"; do
  path=$tree/$f
  (($(stat -c %s -- "$path") <= max_binary)) || found "$f" large-binary
  read -r -a magic < <(bytes "$path" 0 12)
  ((${#magic[@]} >= 4)) || continue
  if [[ ${magic[*]:0:4} == '137 80 78 71' ]]; then
    png_metadata "$path" && found "$f" image-metadata
  elif [[ ${magic[*]:0:3} == '255 216 255' ]]; then
    jpeg_metadata "$path" && found "$f" image-metadata
  elif [[ ${magic[*]:0:4} == '82 73 70 70' && ${magic[*]:8:4} == '87 69 66 80' ]]; then # RIFF....WEBP
    webp_metadata "$path" && found "$f" image-metadata
  fi
done
((images_total <= max_images)) || found docs/images/ images-total

# --- modo privado -------------------------------------------------------
if ((private)); then
  [[ -f $patterns ]] || { echo "leak-scan: private patterns not found: $patterns" >&2; exit 2; }
  # shellcheck source=allowlist.sh
  source "$here/allowlist.sh"
  allowlist_load "$allowlist"
  for f in "${files[@]}"; do
    source_path=$(allowlist_source "$f")
    if ! allowlist_includes "$source_path" || [[ $(allowlist_dest "$source_path") != "$f" ]]; then
      found "$f" outside-allowlist
    fi
  done
  # Padrões: "categoria regex-ERE" por linha, sem diferenciar maiúsculas.
  declare -A by_category=()
  categories=()
  while read -r category regex; do
    [[ -n $category && $category != '#'* ]] || continue
    # Padrão vazio ou ERE inválida: erro de uso, nunca "limpo" (o grep_text
    # engole os erros do grep). A mensagem não cita o padrão.
    if [[ -z $regex ]]; then status=2; else status=0; grep -qiE -e "$regex" <<< '' || status=$?; fi
    ((status <= 1)) || { echo "leak-scan: invalid private pattern in category $category" >&2; exit 2; }
    [[ -n ${by_category[$category]+x} ]] || categories+=("$category")
    by_category[$category]+="${regex}"$'\n'
  done < "$patterns"
  for category in "${categories[@]}"; do
    args=()
    while IFS= read -r regex; do [[ -n $regex ]] && args+=(-e "$regex"); done <<< "${by_category[$category]}"
    report_lines "$category" < <(grep_text -i "${args[@]}" | sort -u)
    # Nomes de arquivo também.
    while IFS= read -r f; do found "$f" "$category (path)"; done \
      < <(printf '%s\n' "${files[@]}" | grep -iE "${args[@]}" || true)
    # E binários (bytes crus): o texto dentro de um arquivo que não é texto
    # também vaza.
    if ((${#binary[@]})); then
      while IFS= read -r -d '' f; do found "$f" "$category (binary)"; done \
        < <(cd "$tree" && printf '%s\0' "${binary[@]}" | xargs -0 -r grep -laiEZ "${args[@]}" -- 2> /dev/null || true)
    fi
  done
  # Referências a arquivos em docs/… que não existem na árvore (relativas à
  # raiz ou ao arquivo). Só arquivos com extensão: diretórios e modelos como
  # "docs/release/v$version.md" ficam de fora (os diretórios internos estão
  # nos padrões). URLs e caminhos absolutos também.
  while IFS=: read -r file line ref; do
    [[ $ref == /* ]] && continue
    target=${ref#./}
    [[ ${target##*/} == ?*.[A-Za-z0-9]* ]] || continue
    [[ -e $tree/$target || -e $tree/$(dirname -- "$file")/$target ]] && continue
    found "$file:$line" missing-doc-link
  done < <(grep_text -e '(^|[^A-Za-z0-9_./:${}-])[A-Za-z0-9_./-]*docs/[A-Za-z0-9_./-]*[A-Za-z0-9_-]([^A-Za-z0-9_./{$-]|$)' \
    | sed -E 's/^([^:]*:[0-9]+:)[^A-Za-z0-9_./-]/\1/; s/[^A-Za-z0-9_-]$//' | sort -u)
fi

if ((${#findings[@]})); then
  printf '%s\n' "${findings[@]}" | sort -u
  summary=$(printf '%s\n' "${findings[@]}" | sort -u | sed -E 's/^.*: //' | sort | uniq -c | awk '{c=$1; $1=""; printf "%s%s=%d", (n++ ? ", " : ""), substr($0, 2), c}')
  echo "leak-scan: $(printf '%s\n' "${findings[@]}" | sort -u | wc -l) finding(s) in $tree: $summary" >&2
  exit 1
fi
echo "leak-scan: clean (${#files[@]} files, $( ((private)) && echo private || echo generic) mode)" >&2
