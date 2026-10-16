<script lang="ts">
  import { t, source, locale } from "$lib/i18n/index.svelte";
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { History } from '@lucide/svelte';
  import { isTauri } from '@tauri-apps/api/core';
  import { DELIVERY, OUTCOME, PROFILE, label, deliveryLabel, ms, historyStats, type HistoryView, type Count } from '$lib/activity';

  const WINDOWS = [7, 30, 90];
  let days = $state(7);
  let stats = $state<HistoryView | null>(null);
  let error = $state<string | null>(null);
  let revision = $state(0);

  onMount(() => {
    if (!isTauri()) return;
    let closed = false;
    let stop: (() => void) | undefined;
    void listen('history-cleared', () => { revision += 1; })
      .then(unlisten => { if (closed) unlisten(); else stop = unlisten; })
      .catch(() => { /* Trocar o período continua relendo o histórico. */ });
    return () => { closed = true; stop?.(); };
  });

  $effect(() => {
    void revision;
    const wanted = days;
    let current = true;
    stats = null;
    error = null;
    if (!isTauri()) { error = source("O histórico só abre no app do hub."); return; }
    historyStats(wanted)
      .then((view) => { if (current) stats = view; })
      .catch((e) => { if (current) error = String(e); });
    return () => { current = false; };
  });

  const count = (rows: Count[], name: string) => rows.find((r) => r.name === name)?.count ?? 0;
  const transcribed = $derived(stats ? count(stats.outcomes, 'transcribed') : 0);
  const pasted = $derived(stats ? stats.deliveries.filter((d) => d.name === 'pasted' || d.name === 'submitted').reduce((n, d) => n + d.count, 0) : 0);
  const failed = $derived(stats ? stats.outcomes.filter((o) => !['transcribed', 'no_speech', 'too_short'].includes(o.name)).reduce((n, o) => n + o.count, 0) : 0);
  const pct = (part: number, whole: number) => (whole ? `${Math.round((part / whole) * 100)}%` : '—');
  const latencies = $derived(stats ? [
    { name: t("Transcrição (ASR)"), p: stats.asr_ms },
    { name: t("IA local"), p: stats.llm_ms },
    { name: t("Soltar → fim do ditado"), p: stats.e2e_ms },
    { name: t("Carga do modelo de transcrição"), p: stats.load_ms },
    { name: t("Carga da IA local"), p: stats.llm_load_ms },
  ].filter((row) => row.p.count > 0) : []);
  const deliveryName = (name: string) => { const [kind, why] = name.split(':', 2); return deliveryLabel(kind, why ?? null); };
  const max = (rows: Count[]) => Math.max(1, ...rows.map((r) => r.count));
</script>

<svelte:head><title>{t("Histórico · FastWisper")}</title></svelte:head>
<h1>{t("Histórico")}</h1>
<p class="subtitle">{t("Uma visão do seu uso, sem guardar o que você disse.")}</p>

<div class="toolbar" role="radiogroup" aria-label={t("Período")}>
  {#each WINDOWS as option}
    <button type="button" role="radio" aria-checked={days === option} class:active={days === option} onclick={() => (days = option)}>
      {option} {t('dias')}
    </button>
  {/each}
</div>

{#if error}
  <p class="note" role="alert">{t(error)}</p>
{:else if !stats}
  <p class="note">{t("Carregando…")}</p>
{:else if stats.empty || stats.total === 0}
  <section class="empty" aria-labelledby="empty-title">
    <History size={32} strokeWidth={1.3} aria-hidden="true" />
    <h2 id="empty-title">{stats.empty ? t("Sem histórico ainda") : t('Nenhum ditado nos últimos {0} dias', stats.days)}</h2>
    <p>{t("Cada ditado registra só desfecho, durações, entrega e a classe do app. Texto ditado e títulos de janela nunca são guardados.")}</p>
  </section>
{:else}
  <section class="tiles" aria-label={t("Resumo")}>
    <div><span>{t("Ditados")}</span><strong>{stats.total.toLocaleString(locale())}</strong></div>
    <div><span>{t("Transcritos")}</span><strong>{transcribed.toLocaleString(locale())}</strong><small>{pct(transcribed, stats.total)} {t("do total")}</small></div>
    <div><span>{t("Colados direto")}</span><strong>{pct(pasted, transcribed)}</strong><small>{pasted.toLocaleString(locale())} {t("de")} {transcribed.toLocaleString(locale())}</small></div>
    <div><span>{t("Falhas")}</span><strong>{failed.toLocaleString(locale())}</strong><small>{pct(failed, stats.total)} {t("do total")}</small></div>
  </section>

  {#if latencies.length}
    <section class="card" aria-labelledby="lat-title">
      <h2 id="lat-title">{t("Tempo de resposta")}</h2>
      <table>
        <thead><tr><th scope="col">{t("Etapa")}</th><th scope="col">p50</th><th scope="col">p95</th><th scope="col">{t("Máximo")}</th><th scope="col">{t("Amostras")}</th></tr></thead>
        <tbody>
          {#each latencies as row}
            <tr><th scope="row">{row.name}</th><td>{ms(row.p.p50)}</td><td>{ms(row.p.p95)}</td><td>{ms(row.p.max)}</td><td>{row.p.count.toLocaleString(locale())}</td></tr>
          {/each}
        </tbody>
      </table>
      <p class="help">{t("p50: metade dos ditados foi mais rápida que isso. p95: só 1 em 20 demorou mais.")}</p>
    </section>
  {/if}

  <div class="grid">
    <section class="card" aria-labelledby="outcome-title">
      <h2 id="outcome-title">{t("Desfechos")}</h2>
      <ul class="bars">
        {#each stats.outcomes as row}
          <li title="{label(OUTCOME, row.name)}: {row.count}">
            <span>{label(OUTCOME, row.name)}</span>
            <span class="track"><span class="fill" style="width:{(row.count / max(stats.outcomes)) * 100}%"></span></span>
            <span class="num">{row.count.toLocaleString(locale())}</span>
          </li>
        {/each}
      </ul>
    </section>
    <section class="card" aria-labelledby="delivery-title">
      <h2 id="delivery-title">{t("Entrega dos transcritos")}</h2>
      <ul class="bars">
        {#each stats.deliveries as row}
          <li title="{deliveryName(row.name)}: {row.count}">
            <span>{deliveryName(row.name)}</span>
            <span class="track"><span class="fill" style="width:{(row.count / max(stats.deliveries)) * 100}%"></span></span>
            <span class="num">{row.count.toLocaleString(locale())}</span>
          </li>
        {/each}
      </ul>
    </section>
  </div>

  {#if stats.apps.length}
    <section class="card" aria-labelledby="apps-title">
      <h2 id="apps-title">{t("Entregas por app")}</h2>
      <table>
        <thead><tr><th scope="col">App</th><th scope="col">{DELIVERY.pasted}</th><th scope="col">{DELIVERY.copied}</th><th scope="col">{DELIVERY.failed}</th><th scope="col">Total</th></tr></thead>
        <tbody>
          {#each stats.apps as row}
            <tr><th scope="row" class="app">{row.app === '-' ? t("Sem janela") : row.app}</th><td>{row.pasted}</td><td>{row.copied}</td><td>{row.failed}</td><td>{row.total}</td></tr>
          {/each}
        </tbody>
      </table>
      <p class="help">{t("Só a classe do app (ex.:")} <code>firefox</code>{t("); o título da janela não é registrado.")}</p>
    </section>
  {/if}

  {#if stats.profiles.some((p) => p.name !== '-')}
    <section class="card" aria-labelledby="profile-title">
      <h2 id="profile-title">{t("Perfil de VRAM nos ditados")}</h2>
      <ul class="bars">
        {#each stats.profiles as row}
          <li title="{label(PROFILE, row.name)}: {row.count}">
            <span>{label(PROFILE, row.name)}</span>
            <span class="track"><span class="fill" style="width:{(row.count / max(stats.profiles)) * 100}%"></span></span>
            <span class="num">{row.count.toLocaleString(locale())}</span>
          </li>
        {/each}
      </ul>
    </section>
  {/if}
{/if}

<style>
  .toolbar { margin-top:32px; display:flex; gap:6px; flex-wrap:wrap; }
  .toolbar button { font:inherit; font-size:13px; color:var(--text-muted); background:transparent; border:1px solid var(--border); border-radius:9px; padding:7px 14px; cursor:pointer; }
  .toolbar button.active { color:var(--text); border-color:var(--accent); background:var(--accent-soft); }
  .toolbar button:focus-visible { outline:2px solid var(--accent); outline-offset:2px; }
  .note { margin-top:24px; color:var(--text-muted); font-size:14px; }
  .empty { margin-top:32px; }
  .tiles { margin-top:24px; display:grid; grid-template-columns:repeat(auto-fit, minmax(150px, 1fr)); gap:12px; }
  .tiles div { border:1px solid var(--border); border-radius:var(--radius); background:var(--surface); padding:16px 18px; display:flex; flex-direction:column; gap:4px; }
  .tiles span { color:var(--text-muted); font-size:12.5px; }
  .tiles strong { font-size:28px; font-weight:500; font-variant-numeric:tabular-nums; letter-spacing:-.5px; }
  .tiles small { color:var(--text-muted); font-size:12px; }
  .grid { display:grid; grid-template-columns:repeat(auto-fit, minmax(min(100%, 300px), 1fr)); gap:16px; margin-top:16px; }
  .grid .card { margin-top:0; }
  .card { margin-top:16px; border:1px solid var(--border); border-radius:var(--radius); background:var(--surface); padding:20px 22px; min-width:0; }
  .card h2 { font-size:15px; margin:0 0 14px; }
  .help { margin:12px 0 0; color:var(--text-muted); font-size:12.5px; line-height:1.5; }
  table { width:100%; border-collapse:collapse; font-size:13.5px; font-variant-numeric:tabular-nums; }
  th, td { padding:8px 6px; text-align:right; border-bottom:1px solid var(--border); }
  thead th { color:var(--text-muted); font-weight:500; font-size:12.5px; }
  th:first-child { text-align:left; font-weight:400; }
  thead th:first-child { font-weight:500; }
  tbody tr:last-child > * { border-bottom:0; }
  .app { overflow-wrap:anywhere; }
  .bars { list-style:none; margin:0; padding:0; display:grid; gap:10px; }
  .bars li { display:grid; grid-template-columns:minmax(110px, 40%) 1fr auto; gap:10px; align-items:center; font-size:13px; }
  .bars li > span:first-child { color:var(--text-muted); overflow-wrap:anywhere; }
  .track { height:8px; border-radius:4px; background:var(--surface-2); overflow:hidden; }
  .fill { display:block; height:100%; min-width:4px; border-radius:4px; background:var(--accent); }
  .num { font-variant-numeric:tabular-nums; min-width:2.5em; text-align:right; }
  code { font-size:12px; background:var(--surface-2); padding:1px 6px; border-radius:5px; }
  @media (max-width:640px) { th, td { padding:7px 4px; font-size:12.5px; } }
</style>
