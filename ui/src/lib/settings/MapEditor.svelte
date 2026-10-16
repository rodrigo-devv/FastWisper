<script lang="ts">
  import { t, source } from "$lib/i18n/index.svelte";
  import { X, Plus, ArrowRight } from '@lucide/svelte';
  import { saveKey } from './store.svelte';
  let { key, entries, placeholder = '', labelledby, disabled = false }: { key: string; entries: Record<string, string>; placeholder?: string; labelledby: string; disabled?: boolean } = $props();
  let from = $state('');
  let to = $state('');
  let error = $state('');
  const rows = $derived(Object.entries(entries));

  async function add(): Promise<void> {
    const f = from.trim();
    const replacement = to.trim();
    if (!f || !replacement) { error = source("Preencha os dois lados."); return; }
    error = '';
    if (await saveKey(key, { ...entries, [f]: replacement })) {
      from = '';
      to = '';
    }
  }
  function remove(f: string): void {
    const next = { ...entries };
    delete next[f];
    void saveKey(key, next);
  }
</script>

<div class="list-editor">
  {#if rows.length}
    <ul class="map-rows" aria-labelledby={labelledby}>
      {#each rows as [f, replacement] (f)}
        <li><span class="mono">{f}</span><ArrowRight size={14} aria-label={t("vira")} /><span class="mono">{replacement}</span><button type="button" class="icon" aria-label="{t("Remover")} {f}" {disabled} onclick={() => remove(f)}><X size={14} /></button></li>
      {/each}
    </ul>
  {:else}
    <p class="empty-line">{t("Nenhuma substituição.")}</p>
  {/if}
  <form class="add-row" onsubmit={(e) => { e.preventDefault(); add(); }}>
    <input type="text" bind:value={from} {placeholder} aria-label={t("Quando a transcrição escrever")} {disabled} spellcheck="false" autocomplete="off" />
    <input type="text" bind:value={to} placeholder={t("ex.: FastWisper")} aria-label={t("Escrever assim")} {disabled} spellcheck="false" autocomplete="off" />
    <button type="submit" class="secondary" disabled={disabled || !from.trim() || !to.trim()}><Plus size={15} />{t("Adicionar")}</button>
  </form>
  {#if error}<p class="field-error" role="alert">{t(error)}</p>{/if}
</div>
