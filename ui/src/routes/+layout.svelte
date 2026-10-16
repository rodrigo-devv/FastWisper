<script lang="ts">
  import { t, followLanguage } from "$lib/i18n/index.svelte";
  import '../app.css';
  import { onMount } from 'svelte';
  import { afterNavigate } from '$app/navigation';
  import { followTheme, themeWarning } from '$lib/backend';
  import { followHubNavigation } from '$lib/navigation';
  import Sidebar from '$lib/shell/Sidebar.svelte';
  import SettingsModal from '$lib/shell/SettingsModal.svelte';
  import { shell, openSettings, closeSettings } from '$lib/shell/shell.svelte';
  let { children } = $props();
  let navigationStarted = false;
  // O destino inicial só abre depois da primeira navegação do SvelteKit:
  // assim o callback da página não fecha o modal recém-aberto pela CLI.
  afterNavigate(({ from }) => {
    if (!navigationStarted) { navigationStarted = true; void followHubNavigation(openSettings); return; }
    if (from && shell.settings !== null) closeSettings();
  });
  onMount(() => { void followTheme(); void followLanguage(); });
</script>

<svelte:head><title>FastWisper</title><meta name="description" content={t("Hub local do FastWisper")} /></svelte:head>
<!-- Com Configurações aberto, o resto fica inerte (foco e leitor de tela só no modal). -->
<div class="shell" inert={shell.settings !== null}>
  <Sidebar />
  <main>{@render children()}</main>
</div>
<SettingsModal />


{#if $themeWarning}<p class="theme-warning" role="status">{t($themeWarning)} <a href="/temas">{t("Escolher tema")}</a></p>{/if}
<style>
  .theme-warning { position:fixed; bottom:16px; right:24px; z-index:60; max-width:420px; margin:0; padding:10px 14px; border:1px solid var(--border); border-radius:10px; background:var(--surface); color:var(--text-muted); font-size:13px; line-height:1.5; }
  .theme-warning a { color:var(--accent); text-decoration:underline; }
</style>
