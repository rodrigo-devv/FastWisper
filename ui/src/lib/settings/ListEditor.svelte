<script lang="ts">
  import { t, source } from "$lib/i18n/index.svelte";
  import { X, Plus } from '@lucide/svelte';
  import { saveKey } from './store.svelte';
  // `save`: grava a lista em outro formato (ex.: uma língua de uma tabela).
  let { key, items, placeholder = '', labelledby, disabled = false, addLabel = "Adicionar item", save = (next: string[]) => saveKey(key, next) }: { key: string; items: string[]; placeholder?: string; labelledby: string; disabled?: boolean; addLabel?: string; save?: (next: string[]) => Promise<boolean> } = $props();
  let entry = $state('');
  let error = $state('');

  async function add(): Promise<void> {
    const value = entry.trim();
    if (!value) return;
    if (items.includes(value)) { error = source("Já está na lista."); return; }
    error = '';
    if (await save([...items, value])) entry = '';
  }
  const remove = (value: string) => save(items.filter((v) => v !== value));
</script>

<div class="list-editor">
  {#if items.length}
    <ul class="chips" aria-labelledby={labelledby}>
      {#each items as item (item)}
        <li class="chip"><span>{item}</span><button type="button" class="icon" aria-label="{t("Remover")} {item}" {disabled} onclick={() => remove(item)}><X size={14} /></button></li>
      {/each}
    </ul>
  {:else}
    <p class="empty-line">{t("Lista vazia.")}</p>
  {/if}
  <form class="add-row" onsubmit={(e) => { e.preventDefault(); add(); }}>
    <input type="text" bind:value={entry} {placeholder} aria-label={t(addLabel)} {disabled} spellcheck="false" autocomplete="off" />
    <button type="submit" class="secondary" disabled={disabled || !entry.trim()}><Plus size={15} />{t("Adicionar")}</button>
  </form>
  {#if error}<p class="field-error" role="alert">{t(error)}</p>{/if}
</div>
