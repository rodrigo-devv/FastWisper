<script lang="ts">
  import { t, source } from "$lib/i18n/index.svelte";
  import { onMount } from 'svelte';
  import { isTauri } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { settings, loadSettings, notify } from '$lib/settings/store.svelte';
  import { listNotes, copyNote, deleteNote, noteTime, type Note } from './notes';
  import NoteRow from './NoteRow.svelte';
  import { watchDaemon } from '$lib/activity';
  import { Search, Rows2, RefreshCw } from '@lucide/svelte';
  let notes = $state<Note[]>([]), query = $state(''), search = $state(false), error = $state(false);
  const enabled = $derived(settings.values['experimental.notes'] === true);
  const filtered = $derived(enabled ? notes.filter(n => n.text.toLocaleLowerCase().includes(query.toLocaleLowerCase())) : []);
  let closed = false, reading = false, again = false;
  async function refresh() {
    if (!isTauri() || !enabled) { notes = []; return; }
    if (reading) { again = true; return; }
    reading = true;
    try { const next = await listNotes(); if (!closed && enabled) { notes = next; error = false; } }
    catch { if (!closed) error = true; }
    finally { reading = false; if (again && !closed) { again = false; void refresh(); } }
  }
  async function copy(id: number) {
    try { await copyNote(id); notify({tone:'ok',text:source("Nota copiada.")}); }
    catch { notify({tone:'err',text:source("Não foi possível copiar a nota.")}); }
  }
  async function remove(id: number) {
    try { await deleteNote(id); await refresh(); }
    catch { notify({tone:'err',text:source("Não foi possível apagar a nota.")}); }
  }
  $effect(() => { if (enabled) void refresh(); else notes = []; });
  let compact = $state(false);
  let scroll: HTMLDivElement;
  let content: HTMLDivElement;
  let fade = $state(false);
  function updateFade() { fade = scroll.scrollHeight - scroll.clientHeight - scroll.scrollTop > 1; }
  onMount(() => {
    closed = false;
    if (!settings.loaded && isTauri()) void loadSettings();
    const stops: (() => void)[] = [];
    const remember = (stop: () => void) => { if (closed) stop(); else stops.push(stop); };
    void watchDaemon(v => { if (v.state === 'idle') void refresh(); }).then(remember);
    if (isTauri()) void listen('notes-changed', () => { void refresh(); }).then(remember);
    const focus = () => { if (isTauri()) void loadSettings().then(refresh); };
    window.addEventListener('focus', focus);
    const timer = setInterval(() => { void refresh(); }, 60_000);
    const observer = new ResizeObserver(updateFade);
    observer.observe(scroll); observer.observe(content); updateFade();
    return () => { closed = true; observer.disconnect(); stops.forEach(stop => stop()); clearInterval(timer); window.removeEventListener('focus',focus); };
  });
</script>

<section class="recent" aria-labelledby="recent-title">
  <header>
    <h2 id="recent-title">{t("RECENTES")}</h2>
    <div class="tools">
      <button type="button" disabled={!enabled} aria-label={t("Buscar notas")} aria-pressed={search} title={t("Buscar notas")} onclick={() => search = !search}><Search size={16} aria-hidden="true" /></button>
      <button type="button" aria-label={t("Visualização compacta")} aria-pressed={compact} title={compact ? t("Exibir lista") : t("Exibir lista compacta")} onclick={() => compact = !compact}><Rows2 size={16} aria-hidden="true" /></button>
      <button type="button" disabled={!enabled} aria-label={t("Atualizar notas")} title={t("Atualizar notas")} onclick={refresh}><RefreshCw size={16} aria-hidden="true" /></button>
    </div>
  </header>
  {#if search && enabled}<input class="search" type="search" aria-label={t("Texto para buscar nas notas recentes")} placeholder={t("Buscar nas notas recentes…")} bind:value={query} />{/if}
  {#if error && enabled}<p class="error" role="alert">{t("Não foi possível ler as notas. Confira se o daemon está ativo.")}</p>{/if}
  <div class="list-area">
    <div class="scroll" bind:this={scroll} onscroll={updateFade}>
      <div bind:this={content}>
        {#if filtered.length}<ul>{#each filtered as note (note.id)}<NoteRow time={noteTime(note.ts)} preview={note.text} words={note.words} language={note.language} {compact} oncopy={() => copy(note.id)} ondelete={() => remove(note.id)} />{/each}</ul>{:else}<p class="empty-notes">{query && enabled ? t("Nenhuma nota encontrada") : t("Nenhuma nota ainda")}</p>{/if}
      </div>
    </div>
    {#if fade}<div class="fade" aria-hidden="true"></div>{/if}
  </div>
</section>

<style>
  .search { margin-top:12px; width:100%; }
  .error { color:var(--err); font-size:13px; }
  .recent { margin-top:28px; flex:1; min-height:0; display:flex; flex-direction:column; }
  header { display:flex; align-items:center; justify-content:space-between; border-bottom:1px solid var(--border); padding-bottom:12px; flex:none; }
  h2 { margin:0; font-size:11.5px; font-weight:600; letter-spacing:1.2px; color:var(--text-muted); }
  .tools { display:flex; align-items:center; gap:14px; }
  button { display:grid; place-items:center; padding:0; width:16px; height:16px; border:0; background:transparent; color:var(--text-muted); cursor:pointer; border-radius:3px; }
  button:hover:not(:disabled), button[aria-pressed=true] { color:var(--text); }
  button:disabled { cursor:default; }
  .list-area { flex:1; min-height:0; position:relative; display:flex; }
  .scroll { flex:1; min-height:0; overflow:auto; scrollbar-width:thin; scrollbar-color:var(--border) transparent; }
  ul { list-style:none; margin:0; padding:0; }
  .empty-notes { margin:0; padding:18px 0; font-size:14px; color:var(--text); }
  .fade { position:absolute; bottom:0; left:0; right:0; height:130px; max-height:100%; pointer-events:none; background:linear-gradient(to bottom, transparent, var(--bg)); }
</style>
