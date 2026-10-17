#!/bin/bash
# Idioma das mensagens do instalador: inglês por padrão, pt-BR quando a
# primeira variável não vazia entre LC_ALL, LC_MESSAGES e LANG começa com
# `pt`. Confere (1) as saídas principais nos dois idiomas, (2) que nenhuma
# mensagem humana fica fora do catálogo e (3) que os dois catálogos têm as
# mesmas chaves. HOME/XDG/XDG_RUNTIME_DIR descartáveis, só --no-activate e
# nenhum comando de serviço, desktop ou rede.
set -euo pipefail
refute() { if "$@"; then echo "asserção negada falhou: $*" >&2; exit 1; fi; }
fail() { echo "i18n: $*" >&2; exit 1; }
repo=$(cd "$(dirname "$0")/../../.." && pwd)
catalog=$repo/packaging/omarchy/messages.sh
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT

# --- (3) Catálogos: mesmas chaves, nada vazio, mesmos `%s` na mesma ordem.
(
  unset LC_ALL LC_MESSAGES LANG
  # shellcheck source=../messages.sh
  source "$catalog"
  diff -u <(printf '%s\n' "${!fw_msg_en[@]}" | sort) <(printf '%s\n' "${!fw_msg_pt[@]}" | sort) \
    || fail 'catálogos en e pt com chaves diferentes'
  for key in "${!fw_msg_en[@]}"; do
    [[ -n ${fw_msg_en[$key]} && -n ${fw_msg_pt[$key]} ]] || fail "mensagem vazia: $key"
    [[ $key =~ ^[a-z][a-z0-9_]*$ ]] || fail "chave fora do padrão: $key"
    en=${fw_msg_en[$key]//%%/} pt=${fw_msg_pt[$key]//%%/}
    [[ $(grep -o '%.' <<< "$en" | tr -d '\n') == "$(grep -o '%.' <<< "$pt" | tr -d '\n')" ]] \
      || fail "placeholders diferentes em $key"
    ! grep -q '%[^s]' <<< "$en$pt" || fail "só %s e %% são aceitos: $key"
  done
)

# --- (2) Varredura: mensagem humana só pelo catálogo.
# Scripts com texto para quem instala; messages.sh é o próprio catálogo.
scan_targets=()
for file in "$repo"/packaging/omarchy/*.sh "$repo/scripts/release/install.sh"; do
  [[ $file == "$catalog" ]] || scan_targets+=("$file")
done
# Exceções justificadas: `echo`/`printf` com texto literal que é dado de
# máquina, nunca mensagem. Formato: arquivo<TAB>linha exata (sem indentação).
exceptions=$(printf '%s\t%s\n' \
  managed.sh 'echo file > "$generation/$index/kind"' \
  install.sh 'echo block > "$generation/$index/kind"' \
  install.sh 'printf '\''\n-- BEGIN fastwisper\ndofile(%s)\n-- END fastwisper\n'\'' "$(quoted_string "$data_root/fastwisper/hyprland.lua")" > "$generation/$index/after"')
# Justificativas: `file`/`block` são o tipo da entrada no ledger (lido por
# restore); o bloco BEGIN/END é o trecho Lua gravado no bindings.lua.
# scan ARQUIVO...: imprime cada violação; sai 1 se houver alguma.
scan() {
  local file line body literal word key found=0
  for file in "$@"; do
    while IFS= read -r line; do
      body=${line#"${line%%[![:space:]]*}"}
      [[ -n $body && $body != \#* ]] || continue
      # Chamadas ao catálogo: a primeira palavra é uma chave existente.
      while read -r word key; do
        [[ -n $word ]] || continue
        if [[ -z ${fw_msg_en[$key]+set} ]]; then
          echo "${file##*/}: chave '$key' fora do catálogo: $body"; found=1
        fi
      done < <(grep -oE '(^|[^A-Za-z0-9_.-])(say_err|say|warn|msg) +[^ ;|&)}]+' <<< "$body" \
        | sed -E 's/^[^A-Za-z]*//; s/["'\'']//g')
      # echo/printf com texto literal: 2+ letras nos argumentos, fora de
      # expansões, escapes e do alvo de redirecionamento para arquivo; o
      # comando termina no primeiro `;`, `|`, `&&` ou `)`.
      if grep -qE '(^|[^A-Za-z0-9_])(echo|printf)( |$)' <<< "$body"; then
        literal=$(sed -E 's/.*(^|[^A-Za-z0-9_])(echo|printf)( |$)/ /' <<< "$body" \
          | sed -E 's/\$\([^)]*\)//g; s/\$\{[^}]*\}//g' \
          | sed -E 's/;.*//; s/ \|.*//; s/ &&.*//; s/\).*//' \
          | sed -E 's/[0-9]?>>? *("[^"]*"|[^ &"][^ ]*)//g' \
          | sed -E 's/\$[A-Za-z_][A-Za-z0-9_]*//g; s/\$[0-9@*#?]//g' \
          | sed -E 's/%[-0-9.]*[sdq%]//g; s/\\[nrt\\"]//g; s/[0-9]?>&[0-9]//g')
        if grep -qE '[[:alpha:]]{2,}' <<< "$literal" \
          && ! grep -qxF -- "${file##*/}"$'\t'"$body" <<< "$exceptions"; then
          echo "${file##*/}: texto fora do catálogo: $body"; found=1
        fi
      fi
    done < "$file"
  done
  return "$found"
}
(
  unset LC_ALL LC_MESSAGES LANG
  # shellcheck source=../messages.sh
  source "$catalog"
  scan "${scan_targets[@]}" > "$scratch/scan" || { cat "$scratch/scan" >&2; fail 'mensagem humana fora do catálogo'; }
  # Toda exceção ainda corresponde a uma linha real (lista sem entradas mortas).
  while IFS=$'\t' read -r file expected; do
    sed 's/^[[:space:]]*//' "$repo/packaging/omarchy/$file" | grep -qxF -- "$expected" \
      || fail "exceção sem linha correspondente em $file: $expected"
  done <<< "$exceptions"
  # Toda chave é usada (o prefixo do aviso é usado pelo próprio catálogo).
  for key in "${!fw_msg_en[@]}"; do
    [[ $key == warning_prefix ]] && continue
    grep -qE "(say_err|say|warn|msg) +$key([^a-z0-9_]|$)" "${scan_targets[@]}" || fail "chave sem uso: $key"
  done
  # A varredura pega texto literal e chave inventada (o teste falha de verdade).
  printf '%s\n' '#!/bin/bash' "echo 'Installing now' >&2" > "$scratch/bad-echo.sh"
  printf '%s\n' '#!/bin/bash' 'printf "Done: %s\n" "$x"' > "$scratch/bad-printf.sh"
  printf '%s\n' '#!/bin/bash' 'warn no_such_key' > "$scratch/bad-key.sh"
  printf '%s\n' '#!/bin/bash' 'printf '\''%s\n'\'' "$x" > "$y"' 'say files_installed' '# echo comentário' > "$scratch/good.sh"
  for bad in bad-echo bad-printf bad-key; do
    refute scan "$scratch/$bad.sh" > /dev/null
  done
  scan "$scratch/good.sh" > /dev/null || fail 'varredura recusou um script sem mensagem literal'
)

# --- (1) Saídas nos dois idiomas, de uma cópia do pacote fora do checkout.
mkdir -p "$scratch/pkg" "$scratch/build" "$scratch/mock" "$scratch/runtime"
chmod 0700 "$scratch/runtime"
cp -r "$repo/packaging" "$scratch/pkg/"
pkg=$scratch/pkg/packaging/omarchy
for binary in fastwisper fastwisperd; do
  printf '#!/bin/bash\nexit 0\n' > "$scratch/build/$binary"
  chmod +x "$scratch/build/$binary"
done
export CALL_LOG=$scratch/calls
for command in systemctl pkill pgrep kill hyprctl omarchy omarchy-shell omarchy-restart-shell curl wget cargo npm node; do
  printf '#!/bin/bash\necho "%s" >> "$CALL_LOG"\nexit 99\n' "$command" > "$scratch/mock/$command"
  chmod +x "$scratch/mock/$command"
done
# iso HOME_NOME [VAR=VALOR...] CMD...: locale limpo e raízes descartáveis.
iso() {
  local home=$scratch/home-$1
  shift
  env -u LC_ALL -u LC_MESSAGES -u LANG -u HYPRLAND_INSTANCE_SIGNATURE -u FASTWISPER_HUB_BUILD_DIR \
    HOME="$home" XDG_CONFIG_HOME="$home/config" XDG_DATA_HOME="$home/data" XDG_STATE_HOME="$home/state" \
    XDG_RUNTIME_DIR="$scratch/runtime" FASTWISPER_BUILD_DIR="$scratch/build" PATH="$scratch/mock:$PATH" "$@"
}
# expect ARQUIVO LINHA...: conteúdo exato.
expect() {
  local file=$1; shift
  diff -u <(printf '%s\n' "$@") "$file" || fail "saída inesperada em ${file##*/}"
}
# scenario NOME VAR=VALOR...: mesma sequência num HOME novo; a saída fica em
# $scratch/out-NOME.*
scenario() {
  local name=$1 home=$scratch/home-$1 rc
  shift
  local out=$scratch/out-$name
  mkdir -p "$home/config/hypr"
  printf '%s\n' 'hl.bind("ALT + SPACE", "exec", "example-app")' > "$home/config/hypr/bindings.lua"
  # Opção desconhecida (--help incluído): uso e código 2, como antes.
  rc=0; iso "$name" "$@" bash "$pkg/install.sh" --help > "$out.help.out" 2> "$out.help.err" || rc=$?
  [[ $rc == 2 && ! -s $out.help.out ]] || fail "$name: --help saiu com $rc"
  rc=0; iso "$name" "$@" bash "$pkg/rollback.sh" > "$out.empty.out" 2> "$out.empty.err" || rc=$?
  [[ $rc == 0 && ! -s $out.empty.err ]] || fail "$name: rollback vazio saiu com $rc"
  # Binário ausente: erro antes de qualquer escrita.
  mv "$scratch/build/fastwisperd" "$scratch/fastwisperd"
  rc=0; iso "$name" "$@" bash "$pkg/install.sh" --no-activate > /dev/null 2> "$out.missing.err" || rc=$?
  mv "$scratch/fastwisperd" "$scratch/build/fastwisperd"
  [[ $rc == 1 ]] || fail "$name: binário ausente saiu com $rc"
  iso "$name" "$@" bash "$pkg/install.sh" --no-activate > "$out.install.out" 2> "$out.install.err"
  # Outra operação segurando o lock.
  exec 8>> "$home/state/fastwisper/packaging/lock"
  flock 8
  rc=0; iso "$name" "$@" bash "$pkg/rollback.sh" > /dev/null 2> "$out.lock.err" || rc=$?
  flock -u 8
  exec 8>&-
  [[ $rc == 1 ]] || fail "$name: lock ocupado saiu com $rc"
  # Desinstalação pela cópia instalada do pacote, de outro diretório: o
  # catálogo vem do caminho do script, não do diretório atual.
  echo '# edição externa' >> "$home/config/systemd/user/fastwisperd.service"
  mkdir -p "$scratch/copy-$name"
  cp -r "$pkg" "$scratch/copy-$name/"
  (cd / && iso "$name" "$@" bash "$scratch/copy-$name/omarchy/uninstall.sh" > "$out.uninstall.out" 2> "$out.uninstall.err")
  [[ ! -e $home/.local/bin/fastwisper ]] || fail "$name: uninstall deixou o binário"
}
usage_en='Usage: install.sh [--offline] [--no-activate] [--adopt-existing] [--no-hub] [--bar-plugin]'
usage_pt='Uso: install.sh [--offline] [--no-activate] [--adopt-existing] [--no-hub] [--bar-plugin]'
check_en() {
  local out=$scratch/out-$1 home=$scratch/home-$1
  expect "$out.help.err" "$usage_en"
  expect "$out.empty.out" 'No managed installation.'
  expect "$out.missing.err" "Missing $scratch/build/fastwisperd; build it before installing."
  expect "$out.install.out" 'Files installed; config, models and external edits preserved.' 'Rollback: packaging/omarchy/rollback.sh'
  expect "$out.install.err" 'External build without FASTWISPER_HUB_BUILD_DIR; continuing without the hub.' \
    "Warning: $home/config/hypr/bindings.lua already binds ALT+SPACE; that bind and the FastWisper one (hotkey.dictate) fire together. Change it with \`fastwisper config set hotkey.dictate <SHORTCUT>\` or remove yours."
  expect "$out.lock.err" 'Another packaging operation is in progress.'
  expect "$out.uninstall.out" 'Conditionally removed; config, models, external choices and services preserved.' \
    'If it was activated, disable the service and the plugin explicitly when safe.'
  expect "$out.uninstall.err" "Preserved (external edit): $home/config/systemd/user/fastwisperd.service"
}
check_pt() {
  local out=$scratch/out-$1 home=$scratch/home-$1
  expect "$out.help.err" "$usage_pt"
  expect "$out.empty.out" 'Nenhuma instalação gerenciada.'
  expect "$out.missing.err" "Falta $scratch/build/fastwisperd; compile antes de instalar."
  expect "$out.install.out" 'Arquivos instalados; config, modelos e edições externas preservados.' 'Rollback: packaging/omarchy/rollback.sh'
  expect "$out.install.err" 'Build externo sem FASTWISPER_HUB_BUILD_DIR; seguindo sem o hub.' \
    "Aviso: $home/config/hypr/bindings.lua já liga ALT+SPACE; esse bind e o do FastWisper (hotkey.dictate) disparam juntos. Troque com \`fastwisper config set hotkey.dictate <ATALHO>\` ou remova o seu."
  expect "$out.lock.err" 'Outra operação de packaging está em andamento.'
  expect "$out.uninstall.out" 'Removido condicionalmente; config, modelos, escolhas externas e serviços preservados.' \
    'Se ativado, desabilite o serviço e o plugin explicitamente quando seguro.'
  expect "$out.uninstall.err" "Preservado (edição externa): $home/config/systemd/user/fastwisperd.service"
}
scenario c LANG=C
check_en c
scenario en LANG=en_US.UTF-8
check_en en
scenario unset
check_en unset
scenario pt LANG=pt_BR.UTF-8
check_pt pt
# Precedência: LC_ALL > LC_MESSAGES > LANG; variável vazia não conta.
scenario all-c LC_ALL=C LANG=pt_BR.UTF-8
check_en all-c
scenario messages-pt LC_MESSAGES=pt_BR.UTF-8 LANG=en_US.UTF-8
check_pt messages-pt
scenario empty-all LC_ALL= LC_MESSAGES= LANG=pt_PT.UTF-8
check_pt empty-all
scenario messages-en LC_ALL= LC_MESSAGES=en_US.UTF-8 LANG=pt_BR.UTF-8
check_en messages-en
# Avisos da ativação (activate.sh) com o mesmo prefixo traduzido.
for spec in 'C|Warning: jq not found; step skipped.' 'pt_BR.UTF-8|Aviso: jq ausente; etapa ignorada.'; do
  got=$(env -u LC_ALL -u LC_MESSAGES LANG="${spec%%|*}" bash -c 'source "$1"; warn tool_missing_skipped jq 2>&1' _ "$catalog")
  [[ $got == "${spec#*|}" ]] || fail "aviso em ${spec%%|*}: '$got'"
done
[[ ! -e $CALL_LOG ]] || fail "comando externo chamado: $(sort -u "$CALL_LOG" | tr '\n' ' ')"
# Atalho do hub: comentário em inglês e tradução pt_BR (Desktop Entry).
grep -qxF 'Comment=Local dictation settings and activity' "$repo/packaging/omarchy/fastwisper.desktop"
grep -qxF 'Comment[pt_BR]=Configurações e atividade do ditado local' "$repo/packaging/omarchy/fastwisper.desktop"
# O tarball da release leva o catálogo junto dos scripts (payload = packaging/).
if git -C "$repo" rev-parse --git-dir > /dev/null 2>&1; then
  # shellcheck source=../../../scripts/release/common.sh
  source "$repo/scripts/release/common.sh"
  release_payload | grep -qxF packaging/omarchy/messages.sh || fail 'messages.sh fora do payload da release'
fi
echo 'i18n: PASS (en padrão, pt-BR por LC_ALL/LC_MESSAGES/LANG, varredura sem texto fora do catálogo, catálogos com as mesmas chaves)'
