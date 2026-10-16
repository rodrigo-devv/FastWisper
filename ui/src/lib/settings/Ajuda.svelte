<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";
  import { invoke } from '@tauri-apps/api/core';
  import { openSettings } from '$lib/shell/shell.svelte';
  import { friendly } from './store.svelte';
  import SettingsGroup from './SettingsGroup.svelte';
  import AdjustmentRow from './AdjustmentRow.svelte';
  import WideButton from './WideButton.svelte';
  let error = $state('');
  async function guide() {
    error = '';
    try { await invoke('open_guide'); } catch (e) { error = friendly(e); }
  }
</script>
<SettingsGroup title={t("Ajuda nesta máquina")}>
  <AdjustmentRow title={t("Guia local")} description={t("Abra o README instalado nesta máquina, com instruções de uso e solução de problemas.")}>
    <WideButton onclick={guide}>{t("Abrir o guia")}</WideButton>
  </AdjustmentRow>
  <AdjustmentRow title={t("Conferência")} description={t("Confira as peças do FastWisper e como corrigir avisos. No terminal, use fastwisper doctor.")}>
    <WideButton onclick={() => openSettings('sistema', 'conferencia')}>{t("Conferir o sistema")}</WideButton>
  </AdjustmentRow>
</SettingsGroup>
{#if error}<p class="field-error" role="alert">{t(error)}</p>{/if}
