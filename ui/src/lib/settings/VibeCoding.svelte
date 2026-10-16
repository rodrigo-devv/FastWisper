<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";
  import { tick } from 'svelte';
  import { ArrowLeft, Hash } from '@lucide/svelte';
  import Field from './Field.svelte';
  import SettingsGroup from './SettingsGroup.svelte';
  import AdjustmentRow from './AdjustmentRow.svelte';
  import WideButton from './WideButton.svelte';
  import DevelopmentNotice from './DevelopmentNotice.svelte';
  import WipSwitchRow from './WipSwitchRow.svelte';
  import { settings } from './store.svelte';
  let editing = $state<'lists' | 'phrases' | null>(null), back = $state<HTMLButtonElement>();
  const terminals = $derived(Array.isArray(settings.values['inject.terminal_apps']) ? (settings.values['inject.terminal_apps'] as string[]).join(', ') : '');
  const phrases = $derived.by(() => {
    const table = (settings.values['inject.send_now_phrases'] ?? {}) as Record<string, string[]>;
    const all = Object.values(table).flatMap((list) => (Array.isArray(list) ? list : []));
    return all.length ? all.map((p) => `“${p}”`).join(', ') : t("Nenhuma frase: o Enter nunca é enviado.");
  });
  async function show(view: 'lists' | 'phrases') { editing = view; await tick(); back?.focus(); }
  async function hide() {
    const from = editing;
    editing = null;
    await tick();
    document.querySelector<HTMLElement>(`#terminais [data-view="${from}"] .wide-button`)?.focus();
  }
</script>
{#if editing === 'phrases'}
  <button type="button" class="ghost back" bind:this={back} onclick={hide}><ArrowLeft size={16} aria-hidden="true" />{t("Voltar aos terminais")}</button>
  <SettingsGroup title={t("Frases do “enviar agora”")}>
    <Field key="inject.send_now_phrases" />
  </SettingsGroup>
{:else if editing === 'lists'}
  <button type="button" class="ghost back" bind:this={back} onclick={hide}><ArrowLeft size={16} aria-hidden="true" />{t("Voltar aos terminais")}</button>
  <SettingsGroup title={t("Terminais e harnesses reconhecidos")}>
    <Field key="inject.terminal_apps" />
    <Field key="inject.harness_apps" />
  </SettingsGroup>
  <SettingsGroup title={t("Colagem por app")}>
    <Field key="inject.paste_apps" />
    <Field key="inject.copy_only_apps" />
    <Field key="inject.notify" />
  </SettingsGroup>
{:else}
  <DevelopmentNotice icon={Hash}>{t("Ditado para quem programa. A área está em construção (WIP): o que já funciona hoje fica no topo.")}</DevelopmentNotice>
  <SettingsGroup title={t("Terminais e harnesses de IA")} id="terminais">
    <Field key="llm.format_in_terminals" />
    <AdjustmentRow title={t("Terminais reconhecidos")} description={t('{0} com a tag terminal. Claude Code, Codex, DeepSeek, Pi e similares no terminal colam com Ctrl+Shift+V; harnesses desktop da lista colam com Ctrl+V.', terminals ? t('{0} e janelas', terminals) : t('Janelas'))}>
      <span data-view="lists"><WideButton onclick={() => show('lists')}>{t("Editar lista")}</WideButton></span>
    </AdjustmentRow>
    <Field key="inject.send_now" />
    <AdjustmentRow title={t("Frases do “enviar agora”")} description={phrases}>
      <span data-view="phrases"><WideButton onclick={() => show('phrases')}>{t("Editar frases")}</WideButton></span>
    </AdjustmentRow>
  </SettingsGroup>
  <SettingsGroup title={t("Em desenvolvimento")}>
    <WipSwitchRow id="wip-code-prompts" title={t("Prompts para agentes de código")} description={t("Formata o ditado como pedido para Claude Code, Codex ou opencode: listas, caminhos e blocos de código.")} />
    <WipSwitchRow id="wip-project-names" title={t("Nomes do projeto")} description={t("Aprende arquivos, funções e símbolos do repositório aberto para escrever certo.")} />
    <AdjustmentRow title={t("Mensagens de commit")} description={t("Dite a mudança e receba uma mensagem no formato Conventional Commits.")} wip><WideButton disabled>{t("Em breve")}</WideButton></AdjustmentRow>
  </SettingsGroup>
{/if}
<style>
  .back { margin-top:28px; }
</style>
