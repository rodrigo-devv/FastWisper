<script lang="ts">
  import { t, locale } from "$lib/i18n/index.svelte";
  import { onMount } from 'svelte';
  import { invoke, isTauri } from '@tauri-apps/api/core';
  import { OFFLINE, PROFILE, label, watchDaemon, type DaemonView, type ModelState, type VramView } from '$lib/activity';
  import { followMeter } from '$lib/meter';
  import { shell, openSettings } from './shell.svelte';
  import Kbd from './Kbd.svelte';
  import { labelsOf, loadHotkeys } from '$lib/settings/hotkeys.svelte';

  // Cartão de estado do menu lateral: o mesmo `DaemonView` do
  // Início. Só estado e números; nunca texto ditado.
  let view = $state<DaemonView>(OFFLINE);
  let totalMb = $state<number | null>(null);
  let level = $state(0);

  type Tone = 'ok' | 'live' | 'busy' | 'off' | 'warn';
  type Bar = 'vram' | 'meter' | 'busy' | 'empty';
  interface Card { tone: Tone; title: string; detail: string; bar: Bar; action: boolean }

  const inVram = (s: ModelState) => s === 'loaded' || s === 'loading';
  function models(vram: VramView | null): string {
    if (!vram) return t("Sem dados da VRAM.");
    const llmInUse = vram.llm !== 'disabled' && vram.llm !== 'external';
    const where = vram.asr === 'loading' || vram.llm === 'loading' ? t("Carregando modelos")
      : inVram(vram.asr) && (inVram(vram.llm) || !llmInUse) ? t("Modelos na VRAM")
      : inVram(vram.asr) ? t("Transcrição na VRAM")
      : inVram(vram.llm) ? t("IA local na VRAM")
      : t("Modelos fora da VRAM");
    return `${where} · ${label(PROFILE, vram.profile)}${vram.game ? ' (jogo aberto)' : ''}`;
  }

  // Atalho configurado (`hotkey.dictate`), relido quando a janela volta ao foco.
  const dictate = $derived(labelsOf('dictate'));
  const card = $derived.by((): Card => {
    if (!view.available) return { tone: 'off', title: t("FastWisper parado"), detail: t("Inicie com systemctl --user start fastwisperd."), bar: 'empty', action: true };
    if (view.state === 'recording') return {
      tone: 'live', title: view.note_session ? t("Gravando nota…") : t("Gravando…"),
      detail: view.note_session ? t("Toque de novo no composer para salvar. Esc cancela.") : view.hands_free ? t("Toque de novo em {0} para transcrever. Esc cancela.", labelsOf('hands_free').join('+')) : t("Solte {0} para transcrever. Esc cancela.", dictate.join('+')),
      bar: 'meter', action: false,
    };
    if (view.state === 'transcribing') return { tone: 'busy', title: t("Transcrevendo…"), detail: view.note_session ? t("A nota será salva nesta máquina.") : t("O texto vai para o app em foco assim que ficar pronto."), bar: 'busy', action: true };
    if (view.asr === 'loading') return { tone: 'busy', title: t("Carregando o modelo…"), detail: t("O ditado fica pronto em alguns segundos."), bar: 'busy', action: true };
    if (view.asr === 'failed') return { tone: 'warn', title: t("Modelo indisponível"), detail: t("A transcrição não carregou. Veja o que falta em Sistema."), bar: 'empty', action: true };
    return { tone: 'ok', title: t("Pronto para ditar"), detail: models(view.vram), bar: 'vram', action: true };
  });

  const recording = $derived(view.available && view.state === 'recording');
  const usedMb = $derived(view.vram?.vram_mb ?? 0);
  /** Largura = VRAM do FastWisper / VRAM total; vazia sem o total. */
  const vramPercent = $derived(totalMb && usedMb ? Math.min(100, (usedMb / totalMb) * 100) : 0);
  const vramLabel = $derived(totalMb ? t("VRAM do FastWisper: {0} de {1} MiB", usedMb.toLocaleString(locale()), totalMb.toLocaleString(locale())) : t("VRAM total indisponível"));

  // Nível do microfone só durante a gravação.
  $effect(() => {
    if (!recording) { level = 0; return; }
    let stop: (() => void) | undefined;
    let closed = false;
    void followMeter((frame) => { level = frame.recording ? frame.level : 0; }).then((unlisten) => { if (closed) unlisten(); else stop = unlisten; });
    return () => { closed = true; stop?.(); };
  });

  onMount(() => {
    let stop: (() => void) | undefined;
    let closed = false;
    void watchDaemon((next) => { view = next; }).then((unlisten) => { if (closed) unlisten(); else stop = unlisten; });
    if (isTauri()) void invoke<number | null>('vram_total').then((mb) => { totalMb = mb; }).catch(() => {});
    void loadHotkeys();
    const refresh = () => { void loadHotkeys(); };
    window.addEventListener('focus', refresh);
    return () => { closed = true; stop?.(); window.removeEventListener('focus', refresh); };
  });
</script>

<section class="status" data-tone={card.tone} aria-labelledby="status-title" aria-live="polite">
  <div class="head"><span class="dot" aria-hidden="true"></span><h2 id="status-title">{card.title}</h2></div>
  <div class="hold">{t("Segure")} <Kbd keys={dictate} /></div>
  {#if card.bar === 'meter'}
    <div class="bar" role="meter" aria-label={t("Nível do microfone")} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(level * 100)}>
      <span class="fill live" style:width="{Math.round(Math.min(1, level) * 100)}%"></span>
    </div>
  {:else if card.bar === 'busy'}
    <div class="bar" aria-hidden="true"><span class="fill busy"></span></div>
  {:else if card.bar === 'vram'}
    <div class="bar" role="meter" aria-label={vramLabel} title={vramLabel} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(vramPercent)}>
      <span class="fill" style:width="{vramPercent}%"></span>
    </div>
  {:else}
    <div class="bar" aria-hidden="true"></div>
  {/if}
  <p class="detail">{card.detail}</p>
  {#if card.action}<button type="button" class="show" onclick={() => openSettings('sistema')}>{t("Ver estado")}</button>{/if}
</section>

<style>
  .status { display:flex; flex-direction:column; align-items:flex-start; gap:10px; padding:16px; border-radius:14px; background:var(--bg); border:1px solid var(--border); }
  .head { display:flex; align-items:center; gap:8px; }
  .dot { width:8px; height:8px; border-radius:50%; flex:none; background:var(--ok); }
  [data-tone=live] .dot { background:var(--err); }
  [data-tone=busy] .dot { background:var(--accent); }
  [data-tone=off] .dot { background:var(--text-muted); }
  [data-tone=warn] .dot { background:var(--warn); }
  h2 { margin:0; font-size:14px; font-weight:500; line-height:normal; }
  .hold { display:flex; align-items:center; gap:5px; color:var(--text-muted); font-size:13px; }
  .bar { position:relative; width:100%; height:4px; border-radius:2px; background:var(--surface-2); overflow:hidden; }
  .fill { position:absolute; inset:0 auto 0 0; background:var(--accent); }
  .fill.live { transition:width 60ms linear; }
  .fill.busy { width:30%; animation:slide 1.2s ease-in-out infinite; }
  @keyframes slide { from { left:-30%; } to { left:100%; } }
  .detail { margin:0; color:var(--text-muted); font-size:12.5px; line-height:18px; }
  .show { font:inherit; font-size:13px; color:var(--text); background:var(--surface-2); border:0; border-radius:9px; padding:7px 12px; cursor:pointer; }
  .show:hover { background:color-mix(in srgb, var(--surface-2) 88%, var(--text)); }
  @media (prefers-reduced-motion: reduce) {
    .fill.live { transition:none; }
    .fill.busy { animation:none; left:0; }
  }
</style>
