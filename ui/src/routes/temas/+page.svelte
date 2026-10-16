<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";
  import { onMount } from 'svelte';
  import { isTauri } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { Check, Palette } from '@lucide/svelte';
  import { getOmarchyThemes, getThemeColors, type OmarchyTheme, type ThemeColors } from '$lib/backend';
  import ThemePreview from '$lib/themes/ThemePreview.svelte';
  import { settings, loadSettings, saveKey, friendly } from '$lib/settings/store.svelte';
  const modes = $derived([{ value: 'auto', label: t("Automático"), detail: t("Acompanha o Omarchy") }, { value: 'dark', label: t("Escuro"), detail: t("Paleta do FastWisper") }, { value: 'light', label: t("Claro"), detail: t("Paleta do FastWisper") }] as const);
  let themes = $state<OmarchyTheme[]>([]);
  let autoColors = $state<ThemeColors | null>(null);
  let loading = $state(true);
  let error = $state('');
  const chosen = $derived(settings.values['hub.theme'] ?? 'auto');
  const busy = $derived(!settings.loaded || !!settings.error || !!settings.saving['hub.theme']);
  async function refresh() {
    try {
      const [catalog, colors] = await Promise.all([getOmarchyThemes(), getThemeColors()]);
      themes = catalog; autoColors = colors; error = '';
    } catch (e) { error = friendly(e); }
    finally { loading = false; }
  }
  function navigateCards(event: KeyboardEvent) {
    if (!['ArrowRight', 'ArrowDown', 'ArrowLeft', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
    const current = event.currentTarget as HTMLButtonElement;
    const cards = Array.from(current.closest('.gallery')?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? []);
    if (!cards.length) return;
    event.preventDefault();
    const index = cards.indexOf(current);
    const next = event.key === 'Home' ? 0 : event.key === 'End' ? cards.length - 1 : (index + (['ArrowRight', 'ArrowDown'].includes(event.key) ? 1 : -1) + cards.length) % cards.length;
    cards[next].focus(); cards[next].click();
  }
  onMount(() => {
    void Promise.all([loadSettings(), refresh()]);
    let disposed = false;
    let unlisten: (() => void) | undefined;
    if (isTauri()) void listen('theme-changed', () => { void Promise.all([loadSettings(), refresh()]); }).then(fn => { if (disposed) fn(); else unlisten = fn; }).catch(() => {});
    return () => { disposed = true; unlisten?.(); };
  });
</script>
<svelte:head><title>{t("Temas · FastWisper")}</title></svelte:head>
<header><div class="title"><Palette size={24} strokeWidth={1.7} aria-hidden="true" /><h1>{t("Temas")}</h1></div><p class="subtitle">{t("Escolha as cores do seu hub. O tema do sistema continua o mesmo.")}</p></header>
<div class="gallery" role="radiogroup" aria-label={t("Tema do hub")}>
  <div class="grid modes">
    {#each modes as mode}
      <button class="card" class:chosen={chosen === mode.value} role="radio" onkeydown={navigateCards} aria-checked={chosen === mode.value} aria-label={mode.label} disabled={busy} onclick={() => saveKey('hub.theme', mode.value)}>
        <ThemePreview mode={mode.value} colors={mode.value === 'auto' ? autoColors : null} />
        <span class="label">{mode.label}{#if chosen === mode.value}<Check size={15} aria-hidden="true" />{/if}</span><span class="detail">{mode.detail}</span>
      </button>
    {/each}
  </div>
  {#if themes.length}
    <div class="section"><h2>OMARCHY</h2><span>{themes.length} {t("temas instalados")}</span></div>
    <div class="grid installed">
      {#each themes as theme (theme.name)}
        <button class="card" class:chosen={chosen === `omarchy:${theme.name}`} role="radio" onkeydown={navigateCards} aria-checked={chosen === `omarchy:${theme.name}`} aria-label={theme.name} disabled={busy} onclick={() => saveKey('hub.theme', `omarchy:${theme.name}`)}>
          <ThemePreview colors={theme.colors} /><span class="label">{theme.name}{#if chosen === `omarchy:${theme.name}`}<Check size={15} aria-hidden="true" />{/if}</span>
        </button>
      {/each}
    </div>
  {/if}
</div>
{#if loading}<p class="hint" role="status">{t("Lendo temas…")}</p>{/if}
{#if error || settings.error}<p class="hint" role="alert">{t("Não foi possível carregar os temas:")} {t(error || settings.error)}</p>{/if}
{#if settings.errors['hub.theme']}<p class="hint" role="alert">{t(settings.errors['hub.theme'])}</p>{/if}
{#if settings.saving['hub.theme']}<p class="hint" role="status">{t("Salvando tema…")}</p>{/if}
<style>
  .title { display:flex; align-items:center; gap:12px; }
  .title :global(svg) { color:var(--accent); }
  .gallery { margin-top:28px; }
  /* Cartões com largura quase fixa: em tela cheia ganham colunas, não esticam. */
  .grid { display:grid; grid-template-columns:repeat(auto-fill,minmax(min(220px,100%),1fr)); gap:16px; }
  .card { min-width:0; appearance:none; text-align:left; font-family:inherit; padding:8px; border:2px solid transparent; border-radius:13px; background:var(--surface); color:var(--text); cursor:pointer; box-shadow:inset 0 0 0 1px var(--border); }
  .card.chosen { border-color:var(--accent); box-shadow:none; }
  .card:hover { background:var(--surface-2); }
  .card:disabled { cursor:wait; }
  .label { display:flex; align-items:center; justify-content:space-between; gap:6px; font-size:14px; font-weight:500; padding:12px 4px 4px; overflow-wrap:anywhere; }
  .label :global(svg) { color:var(--accent); flex:none; }
  .detail { display:block; color:var(--text-muted); font-size:12px; padding:0 4px 4px; }
  .section { display:flex; justify-content:space-between; align-items:center; gap:12px; margin:28px 0 16px; padding-bottom:12px; border-bottom:1px solid var(--border); }
  h2 { margin:0; font-size:11.5px; font-weight:600; letter-spacing:1.2px; color:var(--text-muted); }
  .section > span, .hint { color:var(--text-muted); font-size:12.5px; }
</style>
