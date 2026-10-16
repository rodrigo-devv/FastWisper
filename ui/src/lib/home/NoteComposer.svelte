<script lang="ts">
  import { t, source } from "$lib/i18n/index.svelte";
  import { invoke } from '@tauri-apps/api/core';
  import { Lock, Mic, Square } from '@lucide/svelte';
  import { openSettings } from '$lib/shell/shell.svelte';
  import { settings, notify } from '$lib/settings/store.svelte';
  import { OFFLINE, watchDaemon, type DaemonView } from '$lib/activity';
  import { onMount } from 'svelte';
  import Waveform from './Waveform.svelte';
  import Kbd from '$lib/shell/Kbd.svelte';
  import { labelsOf, loadHotkeys } from '$lib/settings/hotkeys.svelte';
  let view = $state<DaemonView>(OFFLINE), busy = $state(false), seconds = $state(0);
  const enabled = $derived(settings.values['experimental.notes'] === true);
  const recording = $derived(view.note_session && view.state === 'recording');
  const processing = $derived(view.note_session && view.state === 'transcribing');
  $effect(() => {
    if (!recording) { seconds = 0; return; }
    const started = Date.now();
    const timer = setInterval(() => seconds = Math.floor((Date.now() - started) / 1000), 250);
    return () => clearInterval(timer);
  });
  onMount(() => {
    let closed = false, stop: (() => void) | undefined;
    void watchDaemon(v => view = v).then(unlisten => { if (closed) unlisten(); else stop = unlisten; });
    void loadHotkeys();
    return () => { closed = true; stop?.(); };
  });
  async function action(command: 'note_start' | 'note_save' | 'note_cancel') {
    if (busy) return;
    busy = true;
    try { await invoke(command); }
    catch { notify({tone:'err',text:source("Não foi possível concluir a ação de nota. Confira o daemon e o microfone.")},6000); }
    finally { busy = false; }
  }
  function cancel(event: KeyboardEvent) {
    if (event.key === 'Escape' && view.note_session && !event.defaultPrevented) { event.preventDefault(); void action('note_cancel'); }
  }
</script>
<svelte:window onkeydown={cancel} />
<section class="composer" aria-label={enabled ? t("Nota rápida") : t("Nota rápida: Notas desligado")}>
  <div class="card" class:recording>
    <div class="content">
      {#if recording}<p class="live">{t("Gravando a nota… toque de novo para salvar. Esc cancela.")}</p><Waveform />
      {:else if processing}<p>{t("Transcrevendo a nota…")}</p>
      {:else if enabled}<p>{t("Para ideias rápidas: faça uma nota com a sua voz")}</p><p class="hint">{t("Em qualquer app, segure")} <Kbd keys={labelsOf('dictate')} /> {t("para ditar.")}</p>
      {:else}<p>{t("Ative Notas por voz em")} <button class="settings-link" type="button" onclick={() => openSettings('experimental')}>{t("Configurações › Experimental")}</button></p>{/if}
    </div>
    <button class="record" class:stop={recording} type="button" disabled={(!enabled && !recording) || !view.available || busy || processing || (view.state !== 'idle' && !recording)} aria-label={recording ? t("Salvar nota") : enabled ? t("Gravar nota") : t("Gravar nota (Notas desligado)")} onclick={() => action(recording ? 'note_save' : 'note_start')}>
      {#if recording}<Square size={13} fill="currentColor" aria-hidden="true" />{:else}<Mic size={16} aria-hidden="true" />{/if}
    </button>
  </div>
  <p class="privacy"><Lock size={13} aria-hidden="true" />{t("Notas ficam só nesta máquina. Áudio nunca é guardado.")}{#if recording}<span class="elapsed">{Math.floor(seconds / 60)}:{String(seconds % 60).padStart(2,'0')}</span>{/if}</p>
  {#if enabled && settings.values['llm.enabled'] === true && settings.values['llm.remote_consent'] === true}<p class="remote">{t("A formatação usa o provider remoto que você autorizou, enviando só o texto.")}</p>{/if}
</section>
<style>
  .composer { flex:none; padding-top:12px; display:flex; flex-direction:column; gap:10px; }
  .card { height:96px; box-sizing:border-box; padding:16px 16px 16px 18px; display:flex; gap:12px; align-items:flex-start; border:1px solid var(--border); border-radius:16px; background:var(--surface); box-shadow:0 10px 30px rgb(0 0 0 / .35); }
  .card.recording { border:1.5px solid var(--accent); }
  .content { flex:1; min-width:0; display:flex; flex-direction:column; gap:0; }
  .card p { margin:0; color:var(--text-muted); font-size:14px; line-height:1.5; }
  .card p.live { color:var(--text); }
  .card p.hint { display:flex; align-items:center; gap:6px; margin-top:6px; font-size:12.5px; }
  .settings-link { padding:0; border:0; font:inherit; color:inherit; text-decoration:underline; text-underline-offset:3px; background:transparent; cursor:pointer; }
  .settings-link:hover { color:var(--text); }
  .record { flex:none; display:grid; place-items:center; width:32px; height:32px; padding:0; border:0; border-radius:50%; background:var(--text); color:var(--bg); cursor:pointer; }
  .record:disabled { opacity:.4; cursor:not-allowed; }
  .record.stop { background:var(--err); color:white; }
  .privacy { margin:0; display:flex; justify-content:center; align-items:center; gap:10px; font-size:12.5px; line-height:normal; color:var(--text-muted); text-align:center; }
  .privacy :global(svg) { flex:none; }
  .elapsed { font-variant-numeric:tabular-nums; color:var(--text); }
  .remote { margin:0; text-align:center; font-size:12px; color:var(--warn); }
</style>
