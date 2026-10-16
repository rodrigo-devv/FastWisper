<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";
  import { onMount } from 'svelte';
  import { isTauri } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { historyStats, type HistoryView } from '$lib/activity';
  import HomeStats from '$lib/home/HomeStats.svelte';
  import RecentNotes from '$lib/home/RecentNotes.svelte';
  import NoteComposer from '$lib/home/NoteComposer.svelte';

  let stats = $state<HistoryView | null>(null);
  let error = $state(false);
  let loading = false;
  let closed = false;
  let refreshAgain = false;
  async function refreshStats() {
    if (!isTauri()) return;
    if (loading) { refreshAgain = true; return; }
    loading = true;
    try {
      const next = await historyStats(90);
      if (!closed) { stats = next; error = false; }
    } catch { if (!closed) error = true; }
    finally {
      loading = false;
      if (refreshAgain && !closed) { refreshAgain = false; void refreshStats(); }
    }
  }
  onMount(() => {
    closed = false;
    void refreshStats();
    const timer = setInterval(() => { void refreshStats(); }, 60_000);
    const focus = () => { void refreshStats(); };
    window.addEventListener('focus', focus);
    const stops: (() => void)[] = [];
    if (isTauri()) {
      for (const event of ['daemon-status', 'history-cleared']) {
        void listen(event, () => { void refreshStats(); })
          .then((unlisten) => { if (closed) unlisten(); else stops.push(unlisten); })
          .catch(() => { /* A atualização periódica continua disponível. */ });
      }
    }
    return () => { closed = true; clearInterval(timer); window.removeEventListener('focus', focus); stops.forEach(stop => stop()); };
  });
</script>

<svelte:head><title>{t("Início · FastWisper")}</title></svelte:head>
<div class="home">
  <header class="home-header">
    <!-- Saudação fixa por decisão do dono; futuro encaixe permanece aqui. -->
    <h1>{t("Bem-vindo de volta")}</h1>
    <HomeStats {stats} {error} preview={!isTauri()} />
  </header>
  <RecentNotes />
  <NoteComposer />
</div>

<style>
  .home { line-height:normal; height:100%; min-height:0; display:flex; flex-direction:column; }
  .home-header { display:flex; align-items:center; justify-content:space-between; gap:16px; flex:none; }
  h1 { flex:none; line-height:normal; }
  @media (max-width:1050px) { .home-header { flex-wrap:wrap; } }
  @media (max-height:450px) { .home { min-height:320px; } }
</style>
