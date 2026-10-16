<script lang="ts">
  import { t, source } from "$lib/i18n/index.svelte";
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { ShieldCheck } from '@lucide/svelte';
  import Field from './Field.svelte';
  import SettingsGroup from './SettingsGroup.svelte';
  import AdjustmentRow from './AdjustmentRow.svelte';
  import WideButton from './WideButton.svelte';
  import ConfirmAction from './ConfirmAction.svelte';
  import { fieldDef } from '$lib/settings/fields';
  import { settings, apply, notify, friendly } from '$lib/settings/store.svelte';

  interface CloudState { cloud: boolean; disabled: boolean | null; error: string | null }
  const endpointDef = $derived(fieldDef('llm.endpoint'));
  const consentDef = $derived(fieldDef('llm.remote_consent'));
  const keyDef = $derived(fieldDef('llm.api_key_file'));

  let cloud = $state<CloudState | null>(null);
  let cloudError = $state('');
  let applying = $state(false);
  let endpoint = $state('');
  let model = $state('');
  let keyFile = $state('');
  let consent = $state(false);
  let formError = $state('');
  let confirmNotes = $state(false);
  async function clearNotes() {
    clearing = true;
    try { await invoke('notes_clear'); confirmNotes = false; notify({tone:'ok',text:source("Notas apagadas.")}); }
    catch { notify({tone:'err',text:source("Não foi possível apagar as notas.")},6000); }
    finally { clearing = false; }
  }
  let showProvider = $state(false), showFacts = $state(false), confirmClear = $state(false), clearing = $state(false);
  async function clearHistory() {
    clearing = true;
    try { await invoke('history_clear'); confirmClear = false; notify({tone:'ok',text:source("Histórico de uso apagado.")}); }
    catch(e) { notify({tone:'err',text:source("Não foi possível apagar: {0}", friendly(e))},6000); }
    finally { clearing = false; }
  }

  const currentEndpoint = $derived(String(settings.values['llm.endpoint'] ?? ''));
  const engine = $derived(String(settings.values['llm.engine'] ?? 'ollama'));
  const isLoopback = (url: string) => /^https?:\/\/(127\.\d{1,3}\.\d{1,3}\.\d{1,3}|\[::1\])(:\d+)?(\/|$)/.test(url.trim());
  const host = (url: string) => { try { return new URL(url).host; } catch { return url; } };
  const remoteActive = $derived(settings.values['llm.enabled'] === true && engine === 'openai_compatible' && settings.values['llm.remote_consent'] === true && !isLoopback(currentEndpoint));
  const localOllama = $derived(engine === 'ollama' && isLoopback(currentEndpoint));
  // Com provider remoto ativo o texto sai da máquina: o modo offline nunca aparece ligado.
  const offline = $derived(!remoteActive && settings.values['llm.ollama_cloud'] === false);
  const formRemote = $derived(endpoint.trim() !== '' && !isLoopback(endpoint));

  async function loadCloud(): Promise<void> {
    try {
      cloud = await invoke<CloudState>('ollama_cloud');
      settings.values['llm.ollama_cloud'] = cloud.cloud;
      cloudError = '';
    } catch (error) {
      cloudError = friendly(error);
    }
  }

  async function setCloud(online: boolean): Promise<void> {
    applying = true;
    try {
      cloud = await invoke<CloudState>('ollama_cloud_set', { online });
      settings.values['llm.ollama_cloud'] = cloud.cloud;
      cloudError = '';
      notify({ tone: 'ok', text: online ? source("Nuvem do Ollama ligada.") : source("Nuvem do Ollama desligada.") });
    } catch (error) {
      const message = friendly(error);
      await loadCloud();
      cloudError = message;
      notify({ tone: 'err', text: source("Não foi possível mudar a nuvem: {0}", message) }, 8000);
    } finally {
      applying = false;
    }
  }

  async function enableRemote(): Promise<void> {
    const url = endpoint.trim();
    if (!/^https?:\/\//.test(url)) { formError = source("Use um endereço http:// ou https://."); return; }
    if (formRemote && !url.startsWith('https://')) { formError = source("Fora desta máquina, só https://."); return; }
    if (formRemote && !consent) { formError = source("Marque o consentimento para mandar o texto para fora."); return; }
    if (!model.trim()) { formError = source("Informe o nome do modelo no provider."); return; }
    formError = '';
    const ok = await apply([
      ['llm.engine', 'openai_compatible'],
      ['llm.endpoint', url],
      ['llm.model', model.trim()],
      ['llm.api_key_file', keyFile.trim()],
      ['llm.remote_consent', formRemote]
    ]);
    if (ok) { consent = false; showProvider = false; }
  }

  async function backToLocal(): Promise<void> {
    const changes: [string, unknown][] = [
      ['llm.engine', 'ollama'],
      ['llm.endpoint', settings.defaults['llm.endpoint']],
      ['llm.model', settings.defaults['llm.model']],
      ['llm.remote_consent', false]
    ];
    if (settings.values['performance.game_llm'] === 'remote') changes.push(['performance.game_llm', 'local']);
    await apply(changes);
  }

  onMount(() => { void loadCloud(); });
</script>
<SettingsGroup>
  <Field key="llm.ollama_cloud" help={localOllama ? t("Desliga a nuvem e a busca web do Ollama desta máquina. Mudar reinicia o Ollama e descarrega os modelos. Serviços do sistema podem pedir sua senha via polkit.") : remoteActive ? t('Desligado: o texto ditado vai para {0}. Volte ao Ollama local para ficar offline.', host(currentEndpoint)) : t("Não se aplica: a formatação não usa o Ollama desta máquina.")}>
    <button type="button" role="switch" class="switch" aria-label={t("Modo 100% offline")} aria-checked={offline} disabled={!localOllama || applying} onclick={() => setCloud(settings.values['llm.ollama_cloud'] !== true)}><span></span></button>
    {#if applying}<span class="saving" role="status">{t("salvando…")}</span>{/if}
    {#if localOllama && (cloud?.error || cloudError)}<p class="field-error" role="alert">Ollama: {t(cloud?.error ?? cloudError)}</p>
    {:else if localOllama && cloud?.disabled === cloud?.cloud && cloud?.disabled != null}<p class="muted">{t("O estado do Ollama difere da escolha salva.")}</p>{/if}
  </Field>
  <Field key="inject.clipboard_history" />
  <AdjustmentRow title={t("Apagar o histórico de uso")} description={t("Remove as estatísticas e os registros de ditado. O histórico não contém o texto ditado.")}><WideButton danger onclick={() => confirmClear = true}>{t("Apagar")}</WideButton></AdjustmentRow>
  {#if settings.values['experimental.notes'] === true}<AdjustmentRow title={t("Apagar todas as notas")} description={t("Remove o texto das notas salvas nesta máquina. Esta ação não pode ser desfeita.")}><WideButton danger onclick={() => confirmNotes = true}>{t("Apagar notas")}</WideButton></AdjustmentRow>{/if}
  <AdjustmentRow title={t("Provider remoto")} description={engine !== 'ollama' ? t('{0} {1}. Nunca o áudio, o título da janela ou o app.', remoteActive ? t('O texto é enviado para') : t('Provider configurado em'), host(currentEndpoint)) : t("Opcional. Envia só o texto para o serviço escolhido, com seu consentimento. Se falhar, ficam as regras.")}>
    {#snippet metadata()}
      <span class="badge">{t("exige reinício")}</span>
      {#if settings.pendingRestart.some(key => key.startsWith('llm.'))}<span class="badge warn">{t("reinício pendente")}</span>{/if}
      {#if settings.saving['llm.engine']}<span class="saving" role="status">{t("salvando…")}</span>{/if}
    {/snippet}
    {#if engine !== 'ollama'}<WideButton disabled={settings.saving['llm.engine']} onclick={backToLocal}>{t("Voltar ao Ollama local")}</WideButton>{/if}
    <WideButton onclick={() => showProvider = !showProvider}>{showProvider ? t("Fechar formulário") : t("Configurar")}</WideButton>
  </AdjustmentRow>
</SettingsGroup>
{#if showProvider}
  <SettingsGroup title={t("Configurar provider remoto")}>
    <div class="provider-panel">
      <p class="muted">{t("A configuração exige reinício. Fora desta máquina, só HTTPS e consentimento explícito. A chave nunca aparece no hub.")}</p>
    <form class="remote-form" onsubmit={(e) => { e.preventDefault(); void enableRemote(); }}>
      <fieldset disabled={settings.saving['llm.engine']}>
      <label for="remoto-endereco">{endpointDef.label}</label>
      <input id="remoto-endereco" type="url" bind:value={endpoint} placeholder="https://api.exemplo.com" spellcheck="false" autocomplete="off" aria-describedby="remoto-endereco-ajuda" />
      <p class="help" id="remoto-endereco-ajuda">{endpointDef.help} {t("Sem /v1 no fim.")}</p>
      <label for="remoto-modelo">{t("Modelo")}</label>
      <input id="remoto-modelo" type="text" bind:value={model} placeholder={t("ex.: gpt-4o-mini")} spellcheck="false" autocomplete="off" />
      <label for="remoto-chave">{keyDef.label}</label>
      <input id="remoto-chave" type="text" bind:value={keyFile} placeholder="~/.config/fastwisper/chave-api" spellcheck="false" autocomplete="off" aria-describedby="remoto-chave-ajuda" />
      <p class="help" id="remoto-chave-ajuda">{keyDef.help}</p>
      {#if formRemote}
        <label class="consent">
          <input type="checkbox" bind:checked={consent} />
          <span><strong>{consentDef.label}:</strong> {t("entendo que o texto que eu ditar vai para")} <strong>{host(endpoint)}</strong>{t(", fora desta máquina, e fica sujeito às regras desse serviço.")}</span>
        </label>
      {/if}
      {#if formError}<p class="field-error" role="alert">{t(formError)}</p>{/if}
      <button type="submit" class="primary" disabled={!endpoint.trim() || (formRemote && !consent)}>{t("Usar este provider")}</button>
      </fieldset>
    </form>
    </div>
  </SettingsGroup>
{/if}
<div class="privacy-footer"><button type="button" class="secondary" aria-expanded={showFacts} onclick={() => showFacts = !showFacts}><ShieldCheck size={16} />{t("O que fica nesta máquina")}</button></div>
{#if showFacts}
  <ul class="facts" aria-label={t("O que fica nesta máquina")}>
    <li><ShieldCheck size={16} />{t("O áudio nunca é gravado em disco nem sai desta máquina.")}</li>
    <li><ShieldCheck size={16} />{t("O texto ditado não é guardado: o último fica 2 minutos só na memória, para colar de novo.")}</li>
    {#if settings.values['experimental.notes'] === true}<li><ShieldCheck size={16} />{t("Notas por voz guardam texto localmente, só quando você grava pelo Início. A formatação segue o provider consentido.")}</li>{/if}
    <li><ShieldCheck size={16} />{t("O histórico guarda só métricas (tempo, palavras, app de destino), nunca o texto.")}</li>
  </ul>
{/if}
{#if confirmNotes}<ConfirmAction title={t("Apagar todas as notas?")} description={t("Todas as notas locais serão removidas. Esta ação não pode ser desfeita.")} busy={clearing} onconfirm={clearNotes} oncancel={() => confirmNotes = false} />{/if}
{#if confirmClear}<ConfirmAction title={t("Apagar o histórico de uso?")} description={t("Todas as métricas e estatísticas serão removidas. Esta ação não pode ser desfeita. Os próximos ditados iniciam um novo histórico.")} busy={clearing} onconfirm={clearHistory} oncancel={() => confirmClear = false} />{/if}
<style>.provider-panel { padding:20px 24px; } .privacy-footer { margin-top:28px; } fieldset { display:grid; gap:6px; border:0; padding:0; margin:0; min-width:0; }</style>
