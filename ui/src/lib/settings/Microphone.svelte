<script lang="ts">
  import { t, source } from "$lib/i18n/index.svelte";
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { followMeter } from '$lib/meter';
  import Field from './Field.svelte';
  import AdjustmentRow from './AdjustmentRow.svelte';
  import WideButton from './WideButton.svelte';
  import { settings, notify, friendly } from './store.svelte';
  interface Microphone { node_name:string; description:string; chosen:boolean }
  let mics = $state<Microphone[]>([]), problem = $state(''), loading = $state(false), testing = $state(false);
  let level = $state(0), recording = $state(false), peak = $state<number | null>(null);
  const device = $derived(String(settings.values['audio.device'] ?? ''));
  const known = $derived(mics.some((m) => m.node_name === device || m.chosen));
  const options = $derived([
    ...(!mics.some(m => m.node_name === device) ? [{value:device, label:device || t("Escolha um microfone")}] : []),
    ...mics.map(m => ({value:m.node_name, label:m.description}))
  ]);
  async function refresh() {
    loading = true;
    try { mics = await invoke<Microphone[]>('microphones'); problem = ''; }
    catch (e) { problem = friendly(e); }
    finally { loading = false; }
  }
  async function test() {
    testing = true; peak = null;
    try { peak = await invoke<number>('mic_test'); }
    catch (e) { notify({tone:'err', text:source("O teste não rodou: {0}", friendly(e))}, 6000); }
    finally { testing = false; }
  }
  onMount(() => {
    void refresh();
    const stop = followMeter(frame => { level = frame.level; recording = frame.recording; });
    return () => { void stop.then(f => f()); };
  });
</script>
<Field key="audio.device" {options} disabled={loading}>
  <button type="button" class="ghost" onclick={refresh} disabled={loading}>{t("Atualizar")}</button>
</Field>
<AdjustmentRow title={t("Testar microfone")} description={t("Grava 3 segundos e cancela. Nada é transcrito nem colado.")}>
  <div class="microphone-controls">
    <div class="meter" role="meter" aria-label={t("Nível do microfone")} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(level * 100)}>
      {#each Array(24) as _, i}<span class:on={recording && level * 24 > i}></span>{/each}
    </div>
    <!-- Fora do repouso o teste é recusado (o cancel dele descartaria o ditado). -->
    <WideButton disabled={testing || !known || recording} onclick={test}>{testing ? t("Gravando 3 s…") : t("Testar microfone")}</WideButton>
  </div>
  {#snippet error()}
    {#if problem}<p class="field-error" role="alert">{t(problem)}</p>
    {:else if !loading && !known}<p class="field-error">{t("O microfone escolhido não está conectado. O ditado avisa em vez de gravar de outro.")}</p>{/if}
    {#if peak !== null}<p class="muted" role="status">{t("Pico")} {Math.round(peak * 100)}%: {peak < .05 ? t("quase nada captado (mudo ou longe?)") : t("o microfone está captando")}.</p>{/if}
  {/snippet}
</AdjustmentRow>
<style>.microphone-controls { display:grid; gap:10px; width:200px; } .meter { width:100%; box-sizing:border-box; height:30px; padding:0 8px; gap:3px; }</style>
