<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";
  import '$lib/settings/settings.css';
  import { onMount, type Snippet } from 'svelte';
  import { RotateCcw, CircleAlert, CircleCheck, Info } from '@lucide/svelte';
  import { settings, loadSettings, restartDaemon } from '$lib/settings/store.svelte';
  import { shell } from '$lib/shell/shell.svelte';
  import { loadHotkeys } from '$lib/settings/hotkeys.svelte';

  // Em volta de tudo o que edita a config (modal de Configurações e páginas
  // de Personalização): carrega os valores, banner de reinício, erro e toast.
  let { children, modal = false }: { children: Snippet; modal?: boolean } = $props();
  const feedback = $derived(modal || shell.settings === null);
  onMount(() => { void loadSettings(); void loadHotkeys(); });
</script>

{#if feedback && settings.pendingRestart.length}
  <div class="banner warn" role="status">
    <RotateCcw size={18} aria-hidden="true" />
    <p>{t("Algumas mudanças só valem depois de reiniciar:")} <span class="mono">{settings.pendingRestart.join(', ')}</span>.</p>
    <button type="button" class="primary" disabled={settings.restarting} onclick={restartDaemon}>{settings.restarting ? t('Reiniciando…') : t("Reiniciar o FastWisper")}</button>
  </div>
{/if}

{#if feedback && settings.error}
  <div class="banner err" role="alert"><CircleAlert size={18} aria-hidden="true" /><p>{t("Não foi possível ler a configuração:")} {t(settings.error)}</p></div>
{/if}

<div class="settings-body" aria-busy={!settings.loaded}>
  {#if settings.loaded && !settings.error}{@render children()}{:else if !settings.loaded}<p class="empty-line">{t("Carregando…")}</p>{/if}
</div>

{#if feedback && settings.notice}
  <div class="toast {settings.notice.tone}" role="status" aria-live="polite">
    {#if settings.notice.tone === 'ok'}<CircleCheck size={16} aria-hidden="true" />{:else if settings.notice.tone === 'info'}<Info size={16} aria-hidden="true" />{:else}<CircleAlert size={16} aria-hidden="true" />{/if}
    <span>{t(settings.notice.text)}</span>
  </div>
{/if}
