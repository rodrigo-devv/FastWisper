#!/bin/bash
# Catálogo das mensagens humanas dos scripts de pacote. Inglês por padrão;
# pt-BR quando a primeira variável não vazia entre LC_ALL, LC_MESSAGES e LANG
# começa com `pt`. Só define funções e tabelas: carregado por managed.sh e
# activate.sh pelo caminho relativo ao script (checkout, tarball ou cópia).
# Modelos são formatos do printf: `%s` recebe os argumentos, `%%` é um `%`.
# Os dois idiomas recebem os argumentos na mesma ordem e têm as mesmas chaves
# (packaging/omarchy/tests/i18n.sh confere).
declare -gA fw_msg_en=(
  [absolute_path_required]='Absolute path required: %s'
  [newline_path_refused]='Path with newline refused.'
  [cr_path_refused]='Path with CR refused.'
  [symlink_dir_refused]='Symlinked directory refused: %s'
  [symlink_state_refused]='Symlinked state refused: %s'
  [lock_busy]='Another packaging operation is in progress.'
  [preserved_symlink_dir]='Preserved (directory replaced by a symlink): %s'
  [preserved_external_mode]='Preserved (external mode): %s'
  [preserved_external_edit]='Preserved (external edit): %s'
  [preserved_later_choice]='Preserved (later choice): %s'
  [preserved_preexisting]='Preserved (pre-existing file): %s'
  [symlink_refused]='Symlink refused: %s'
  [install_usage]='Usage: install.sh [--offline] [--no-activate] [--adopt-existing] [--no-hub] [--bar-plugin]'
  [cargo_missing]='cargo not found: install Rust (rustup) or set FASTWISPER_BUILD_DIR.'
  [building]='Building (cargo build --release)...'
  [build_failed]='Build failed; nothing was installed.'
  [binary_missing]='Missing %s; build it before installing.'
  [hub_unavailable]='Hub unavailable at %s; continuing without the hub.'
  [hub_external_build]='External build without FASTWISPER_HUB_BUILD_DIR; continuing without the hub.'
  [hub_missing_node]='node/npm not found; continuing without the hub (dictation is still available).'
  [hub_missing_webkit]='webkit2gtk-4.1 not found; continuing without the hub (dictation is still available).'
  [hub_building]='Building the local hub...'
  [hub_build_failed]='Hub build failed; continuing without the hub (dictation is still available; --no-hub skips the build).'
  [bindings_symlink]='Lua bindings file is a symlink: %s'
  [legacy_refused]='Migration refused: legacy FastWisper integration not recognized in the bindings.'
  [hotkey_conflict]='%s already binds %s; that bind and the FastWisper one (hotkey.%s) fire together. Change it with `fastwisper config set hotkey.%s <SHORTCUT>` or remove yours.'
  [files_installed]='Files installed; config, models and external edits preserved.'
  [rollback_hint]='Rollback: packaging/omarchy/rollback.sh'
  [no_managed_install]='No managed installation.'
  [rollback_done]='Conditional rollback done; reload/activate explicitly when safe.'
  [uninstall_done]='Conditionally removed; config, models, external choices and services preserved.'
  [uninstall_hint]='If it was activated, disable the service and the plugin explicitly when safe.'
  [warning_prefix]='Warning: '
  [tool_missing_skipped]='%s not found; step skipped.'
  [optional_failed]='failed: %s; copied files stay installed.'
  [setup_failed]='setup failed; fix the config/audio and run install.sh again.'
  [offline_asr_skipped]='offline mode: ASR download skipped.'
  [asr_failed]='ASR preparation failed; run install.sh again.'
  [ollama_cloud_not_applied]='Ollama cloud setting not applied; run `fastwisper ollama apply` in a terminal.'
  [manual_pid_ignored]='manual fastwisperd PID %s ignored: different, missing or unverifiable runtime; no signal sent.'
  [manual_stop_failed]='could not stop the manual fastwisperd.'
  [manual_still_running]='manual fastwisperd still running; service activation postponed.'
  [pgrep_missing]='pgrep not found; stopping a manual daemon skipped.'
  [systemctl_missing]='systemctl not found; service activation skipped.'
  [hotkeys_not_applied]='config shortcuts not applied; run `fastwisper hotkeys apply`.'
  [hypr_new_errors]='Hyprland reported new errors: %s; undoing the block from this installation.'
  [hypr_unavailable]='Hyprland unavailable; block kept and validation skipped.'
  [hypr_missing]='hyprctl or Lua bindings not found; bind validation skipped.'
  [jq_missing]='jq not found; shell plugin check/activation skipped.'
  [shell_restart_failed]='shell restart failed; the active QML stays pending.'
  [shell_rescan_failed]='shell rescan failed; the active QML stays pending.'
  [binds_not_validated]='Installation finished with binds NOT validated; check them in the Hyprland session.'
  [bar_plugin_hint]='Bar widget (optional, separate plugin): omarchy plugin add %s --enable (or install.sh --bar-plugin).'
  [bar_plugin_migrate]='the bar widget is now a separate plugin and fastwisper.dictation no longer gets updates. Migrate with `omarchy plugin add %s --enable` and then `omarchy plugin remove fastwisper.dictation` (or `omarchy plugin disable fastwisper.dictation`).'
  [bar_plugin_offline]='offline mode: bar plugin download skipped; add it later with `omarchy plugin add %s --enable`.'
)
declare -gA fw_msg_pt=(
  [absolute_path_required]='Caminho absoluto obrigatório: %s'
  [newline_path_refused]='Caminho com newline recusado.'
  [cr_path_refused]='Caminho com CR recusado.'
  [symlink_dir_refused]='Diretório symlink recusado: %s'
  [symlink_state_refused]='Estado symlink recusado: %s'
  [lock_busy]='Outra operação de packaging está em andamento.'
  [preserved_symlink_dir]='Preservado (diretório substituído por symlink): %s'
  [preserved_external_mode]='Preservado (modo externo): %s'
  [preserved_external_edit]='Preservado (edição externa): %s'
  [preserved_later_choice]='Preservado (escolha posterior): %s'
  [preserved_preexisting]='Preservado (arquivo preexistente): %s'
  [symlink_refused]='Symlink recusado: %s'
  [install_usage]='Uso: install.sh [--offline] [--no-activate] [--adopt-existing] [--no-hub] [--bar-plugin]'
  [cargo_missing]='cargo ausente: instale o Rust (rustup) ou defina FASTWISPER_BUILD_DIR.'
  [building]='Compilando (cargo build --release)...'
  [build_failed]='Compilação falhou; nada foi instalado.'
  [binary_missing]='Falta %s; compile antes de instalar.'
  [hub_unavailable]='Hub indisponível em %s; seguindo sem o hub.'
  [hub_external_build]='Build externo sem FASTWISPER_HUB_BUILD_DIR; seguindo sem o hub.'
  [hub_missing_node]='node/npm ausente; seguindo sem o hub (o ditado continua disponível).'
  [hub_missing_webkit]='webkit2gtk-4.1 ausente; seguindo sem o hub (o ditado continua disponível).'
  [hub_building]='Compilando o hub local...'
  [hub_build_failed]='Compilação do hub falhou; seguindo sem o hub (o ditado continua disponível; --no-hub pula o build).'
  [bindings_symlink]='Bindings Lua symlink: %s'
  [legacy_refused]='Migração recusada: integração legada FastWisper não reconhecida nos bindings.'
  [hotkey_conflict]='%s já liga %s; esse bind e o do FastWisper (hotkey.%s) disparam juntos. Troque com `fastwisper config set hotkey.%s <ATALHO>` ou remova o seu.'
  [files_installed]='Arquivos instalados; config, modelos e edições externas preservados.'
  [rollback_hint]='Rollback: packaging/omarchy/rollback.sh'
  [no_managed_install]='Nenhuma instalação gerenciada.'
  [rollback_done]='Rollback condicional concluído; recarregue/ative explicitamente quando seguro.'
  [uninstall_done]='Removido condicionalmente; config, modelos, escolhas externas e serviços preservados.'
  [uninstall_hint]='Se ativado, desabilite o serviço e o plugin explicitamente quando seguro.'
  [warning_prefix]='Aviso: '
  [tool_missing_skipped]='%s ausente; etapa ignorada.'
  [optional_failed]='falha em %s; arquivos copiados permanecem instalados.'
  [setup_failed]='setup falhou; corrija a config/áudio e execute install.sh novamente.'
  [offline_asr_skipped]='modo offline: download ASR ignorado.'
  [asr_failed]='preparação ASR falhou; execute install.sh novamente.'
  [ollama_cloud_not_applied]='nuvem do Ollama não ajustada; rode `fastwisper ollama apply` num terminal.'
  [manual_pid_ignored]='fastwisperd manual PID %s ignorado: runtime diferente, ausente ou não verificável; nenhum sinal enviado.'
  [manual_stop_failed]='não foi possível encerrar fastwisperd manual.'
  [manual_still_running]='fastwisperd manual ainda ativo; ativação do serviço adiada.'
  [pgrep_missing]='pgrep ausente; encerramento de daemon manual ignorado.'
  [systemctl_missing]='systemctl ausente; ativação do serviço ignorada.'
  [hotkeys_not_applied]='atalhos da config não aplicados; rode `fastwisper hotkeys apply`.'
  [hypr_new_errors]='Hyprland reportou erros novos: %s; desfazendo o bloco desta instalação.'
  [hypr_unavailable]='Hyprland indisponível; bloco mantido e validação ignorada.'
  [hypr_missing]='hyprctl/bindings Lua ausente; validação dos binds ignorada.'
  [jq_missing]='jq ausente; verificação/ativação dos plugins do shell ignorada.'
  [shell_restart_failed]='restart do shell falhou; QML ativo permanece pendente.'
  [shell_rescan_failed]='rescan do shell falhou; QML ativo permanece pendente.'
  [binds_not_validated]='Instalação concluída com binds NÃO validados; confira na sessão Hyprland.'
  [bar_plugin_hint]='Widget da barra (opcional, plugin separado): omarchy plugin add %s --enable (ou install.sh --bar-plugin).'
  [bar_plugin_migrate]='o widget da barra agora é um plugin separado e o fastwisper.dictation não recebe mais atualizações. Migre com `omarchy plugin add %s --enable` e depois `omarchy plugin remove fastwisper.dictation` (ou `omarchy plugin disable fastwisper.dictation`).'
  [bar_plugin_offline]='modo offline: download do plugin da barra ignorado; adicione depois com `omarchy plugin add %s --enable`.'
)
# en ou pt, conforme a precedência POSIX das variáveis de locale.
msg_lang() {
  local locale=${LC_ALL:-${LC_MESSAGES:-${LANG:-}}}
  if [[ $locale == pt* ]]; then echo pt; else echo en; fi
}
# msg CHAVE [ARGS...]: o texto formatado, sem newline. Chave sem tradução cai
# no inglês; chave desconhecida sai como ela mesma (o teste do catálogo pega).
msg() {
  local key=$1 template
  shift
  if [[ $(msg_lang) == pt && -n ${fw_msg_pt[$key]+set} ]]; then template=${fw_msg_pt[$key]}
  else template=${fw_msg_en[$key]-$key}; fi
  # shellcheck disable=SC2059 # o modelo vem do catálogo, nunca do usuário
  printf -- "$template" "$@"
}
say() { msg "$@"; printf '\n'; }
say_err() { say "$@" >&2; }
warn() { { msg warning_prefix; say "$@"; } >&2; }
