<script lang="ts">
  import { t, source } from "$lib/i18n/index.svelte";
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import type { DoctorSummary } from '$lib/activity';
  import Field from './Field.svelte';
  import SettingsGroup from './SettingsGroup.svelte';
  import AdjustmentRow from './AdjustmentRow.svelte';
  import WideButton from './WideButton.svelte';
  import { settings, saveKey, notify, friendly, restartDaemon } from './store.svelte';
  interface AsrModel { id:string; size_mb:number; note:string; license:string; path:string; installed:boolean; active:boolean }
  let models = $state<AsrModel[]>([]), gpus = $state<string[]>([]), installed = $state<string[]>([]);
  let errors = $state<Record<string,string>>({}), progress = $state<Record<string,number>>({});
  let chosenModel = $state(''), doctor = $state<DoctorSummary|null>(null);
  let checking = $state(false), releasing = $state(false);
  const enabled = $derived(settings.values['llm.enabled'] === true);
  const ollama = $derived(settings.values['llm.engine'] === 'ollama');
  const selected = $derived(models.find(m => m.path === chosenModel));
  const model = $derived(String(settings.values['llm.model'] ?? ''));
  const llmOptions = $derived([...new Set([model, ...installed])].filter(Boolean).map(value => ({value,label:value})));
  const gpu = $derived(String(settings.values['asr.gpu'] ?? 'auto'));
  const gpuOptions = $derived([...new Set(['auto','cpu',...gpus,gpu])].map(value => ({value,label:value === 'auto' ? t("Automático (melhor GPU)") : value === 'cpu' ? t("Só o processador (CPU)") : value})));
  const firstNotice = $derived(doctor?.notices[0]);
  $effect(() => { chosenModel = String(settings.values['asr.model'] ?? ''); });
  async function loadList<T>(command:string, accept:(value:T)=>void) {
    try { accept(await invoke<T>(command)); delete errors[command]; }
    catch(e) { errors[command] = friendly(e); }
  }
  async function refreshModels() { await loadList<AsrModel[]>('asr_models', value => models = value); }
  async function check() {
    checking = true;
    await loadList<DoctorSummary>('doctor_summary', value => doctor = value);
    checking = false;
  }
  async function chooseAsr(path:string) {
    chosenModel = path;
    const item = models.find(m => m.path === path);
    if (item?.installed && await saveKey('asr.model', path)) await refreshModels();
  }
  async function pull() {
    if (!selected) return;
    const item = selected; progress[item.id] = 0;
    try {
      await invoke('asr_model_pull',{id:item.id});
      if (await saveKey('asr.model',item.path)) notify({tone:'ok',text:source("{0} baixado e conferido.", item.id)});
    } catch(e) { notify({tone:'err',text:source("Download falhou: {0}", friendly(e))},8000); }
    finally { delete progress[item.id]; await refreshModels(); }
  }
  async function release() {
    releasing = true;
    try { await invoke('release_vram'); notify({tone:'ok',text:source("Liberação de VRAM solicitada.")}); }
    catch(e) { notify({tone:'err',text:friendly(e)},6000); }
    finally { releasing = false; }
  }
  onMount(() => {
    void refreshModels(); void loadList<string[]>('asr_gpus', value => gpus = value);
    if (ollama) void loadList<string[]>('ollama_models', value => installed = value);
    void check();
    const stop = listen<{id:string;progress:number}>('model-progress',e => progress[e.payload.id] = e.payload.progress);
    return () => { void stop.then(f => f()); };
  });
</script>
<SettingsGroup title={t("Transcrição")}>
  <Field key="asr.model">
    <select aria-label={t("Modelo de transcrição")} value={chosenModel} disabled={!models.length || settings.saving['asr.model']} onchange={e => chooseAsr(e.currentTarget.value)}>
      {#if !models.some(m => m.path === chosenModel)}<option value={chosenModel}>{t("Modelo configurado")}</option>{/if}
      {#each models as item}<option value={item.path}>{item.id}{item.installed ? '' : ` · baixar ${item.size_mb} MB`}</option>{/each}
    </select>
    {#if selected && !selected.installed}<WideButton disabled={progress[selected.id] !== undefined} onclick={pull}>{progress[selected.id] !== undefined ? t('Baixando {0}%…', progress[selected.id]) : t("Baixar e usar")}</WideButton>{/if}
    {#if selected?.installed && chosenModel !== settings.values['asr.model']}<WideButton disabled={settings.saving['asr.model']} onclick={() => chooseAsr(chosenModel)}>{t("Usar modelo")}</WideButton>{/if}
    {#if selected}<span class="muted">{selected.note} · {selected.license}</span>{/if}
    {#if errors['asr_models']}<p class="field-error" role="alert">{t(errors['asr_models'])}</p>{/if}
  </Field>
  <Field key="asr.gpu" options={gpuOptions} />
  {#if errors['asr_gpus']}<p class="inline-error" role="alert">{t(errors['asr_gpus'])}</p>{/if}
</SettingsGroup>
<SettingsGroup title={t("IA local")}>
  <Field key="llm.enabled" />
  <Field key="llm.model" options={ollama ? llmOptions : undefined} disabled={!enabled}>
    {#if !ollama}<input type="text" aria-label={t("Modelo do provider")} value={model} onchange={e => saveKey('llm.model',e.currentTarget.value.trim())} disabled={!enabled} />{/if}
    {#if errors['ollama_models']}<p class="field-error" role="alert">{t(errors['ollama_models'])}. {t('O ditado segue com as regras.')}</p>{/if}
  </Field>
  <Field key="llm.style" disabled={!enabled} /><Field key="llm.timeout_ms" disabled={!enabled} /><Field key="llm.skip_below_words" disabled={!enabled} />
</SettingsGroup>
<SettingsGroup title={t("Memória da GPU (VRAM)")}>
  <Field key="performance.vram" /><Field key="performance.unload_after_secs" /><Field key="performance.game_llm" />
  <AdjustmentRow title={t("Liberar VRAM agora")} description={t("Descarrega os modelos quando não houver um ditado em andamento.")}><WideButton disabled={releasing} onclick={release}>{releasing ? t('Liberando…') : t("Liberar VRAM")}</WideButton></AdjustmentRow>
</SettingsGroup>
<SettingsGroup title={t("Manutenção")} id="conferencia">
  <AdjustmentRow title={t("Conferência")} description={t(errors['doctor_summary'] ?? '') || (firstNotice ? `${t(firstNotice.item)}: ${t(firstNotice.detail)}${firstNotice.hint ? ` ${t(firstNotice.hint)}` : ''}` : t(doctor?.verdict ?? '') || t("Confere as peças do FastWisper e mostra como corrigir avisos."))}>
    <WideButton disabled={checking} onclick={check}>{checking ? t('Conferindo…') : t("Conferir de novo")}</WideButton>
  </AdjustmentRow>
  <AdjustmentRow title={t("Reiniciar o FastWisper")} description={t("Aplica as opções que exigem reinício. Faça isso fora de um ditado.")}><WideButton disabled={settings.restarting} onclick={restartDaemon}>{settings.restarting ? t('Reiniciando…') : t("Reiniciar o FastWisper")}</WideButton></AdjustmentRow>
</SettingsGroup>
<style>
  :global(.settings-modal .adjustment-row:has(#campo-asr-gpu)) { grid-template-columns:minmax(0,1fr) 232px; }
  :global(.settings-modal #campo-asr-gpu) { width:232px; }
  @media(max-width:760px) {
    :global(.settings-modal .adjustment-row:has(#campo-asr-gpu)) { grid-template-columns:minmax(0,1fr); }
    :global(.settings-modal #campo-asr-gpu) { width:100%; }
  }

  .inline-error { color:var(--err); font-size:13px; margin:0; padding:0 24px 16px; }
</style>
