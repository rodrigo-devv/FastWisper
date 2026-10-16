<script lang="ts">
  import { t, locale } from "$lib/i18n/index.svelte";
  import { Flame, Type, Gauge } from '@lucide/svelte';
  import type { HistoryView } from '$lib/activity';
  let { stats, error = false, preview = false }: { stats: HistoryView | null; error?: boolean; preview?: boolean } = $props();
  const value = (n: number | undefined) => error ? '—' : n != null ? n.toLocaleString(locale()) : preview ? '0' : '—';
  const weeks = $derived(`${value(stats?.consecutive_weeks)} ${!error && stats?.consecutive_weeks === 1 ? t("semana") : t("semanas")}`);
  const words = $derived(`${value(stats?.words)} ${!error && stats?.words === 1 ? t("palavra") : t("palavras")}`);
  const ppm = $derived(t("{0} PPM", value(stats?.ppm)));
</script>

<a class="home-stats" href="/historico" aria-label={t('Ver estatísticas: {0}, {1}, {2}', weeks, words, ppm)} title={t("Ver estatísticas")}>
  <span><Flame size={15} aria-hidden="true" />{weeks}</span>
  <i aria-hidden="true"></i>
  <span><Type size={15} aria-hidden="true" />{words}</span>
  <i aria-hidden="true"></i>
  <span><Gauge size={15} aria-hidden="true" />{ppm}</span>
  <span class="tooltip" role="tooltip">{t("Ver estatísticas")}</span>
</a>
{#if error}<span class="sr-only" role="status">{t("Estatísticas indisponíveis. Consulte o Histórico.")}</span>{/if}

<style>
  .home-stats { position:relative; display:flex; align-items:center; gap:12px; padding:7px 14px; border:1px solid var(--border); border-radius:999px; font-size:13.5px; line-height:normal; white-space:nowrap; }
  .home-stats > span:not(.tooltip) { display:flex; align-items:center; gap:6px; font-variant-numeric:tabular-nums; }
  .home-stats :global(svg) { color:var(--accent); flex:none; }
  i { height:14px; width:1px; background:var(--border); }
  .home-stats:hover, .home-stats:focus-visible { background:var(--hover); border-color:var(--text-muted); }
  .tooltip { position:absolute; right:0; top:calc(100% + 8px); padding:6px 10px; border:1px solid var(--border); border-radius:8px; background:var(--surface); color:var(--text-muted); font-size:12px; pointer-events:none; opacity:0; z-index:3; }
  .home-stats:hover .tooltip, .home-stats:focus-visible .tooltip { opacity:1; }
  @media (max-width:520px) { .home-stats { gap:8px; padding:7px 10px; font-size:12px; } }
</style>
