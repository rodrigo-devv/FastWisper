<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";
  import { invoke } from '@tauri-apps/api/core';
  import { friendly } from './store.svelte';
  import SettingsGroup from './SettingsGroup.svelte';
  import AdjustmentRow from './AdjustmentRow.svelte';
  import WideButton from './WideButton.svelte';
  let { version }: { version: string } = $props();
  let error = $state('');
  async function open(url: string) {
    error = '';
    try { await invoke('open_about_link', { url }); } catch (e) { error = friendly(e); }
  }
</script>
<p class="description">{t("Ditado por voz 100% local para Omarchy/Hyprland. Transcreve e formata na própria máquina, sem nuvem por padrão.")}</p>
<SettingsGroup title="FastWisper">
  <AdjustmentRow title={t("Versão")} description={t("Versão do aplicativo, definida no Cargo.")}><span class="version">v{version}</span></AdjustmentRow>
  <AdjustmentRow title={t("Repositório")} description="github.com/rodrigo-devv/FastWisper"><WideButton onclick={() => open('https://github.com/rodrigo-devv/FastWisper')}>{t("Abrir repositório")}</WideButton></AdjustmentRow>
  <AdjustmentRow title={t("Desenvolvedor")} description="github.com/rodrigo-devv"><WideButton onclick={() => open('https://github.com/rodrigo-devv')}>{t("Abrir perfil")}</WideButton></AdjustmentRow>
</SettingsGroup>
<p class="browser-note">{t("Os links abrem no navegador do sistema.")}</p>
{#if error}<p class="field-error" role="alert">{t(error)}</p>{/if}
<style>
  .description { margin-top:28px; color:var(--text-muted); font-size:15px; line-height:1.6; }
  .version { font-size:15px; font-weight:500; }
  .browser-note { margin-top:20px; color:var(--text-muted); font-size:13px; }
</style>
