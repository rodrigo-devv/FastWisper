<script lang="ts">
  import { t, source } from "$lib/i18n/index.svelte";
  import { X, Plus } from '@lucide/svelte';
  import { saveKey } from './store.svelte';
  type Item = { trigger: string; expansion: string };
  let { key, items, labelledby, disabled = false }: { key: string; items: Item[]; labelledby: string; disabled?: boolean } = $props();
  let trigger = $state('');
  let expansion = $state('');
  let error = $state('');

  async function add(): Promise<void> {
    const normalized = trigger.trim();
    if (!normalized || !expansion.trim()) { error = source("Preencha o gatilho e o texto."); return; }
    if (items.some((i) => i.trigger.toLowerCase() === normalized.toLowerCase())) { error = source("Esse gatilho já existe."); return; }
    error = '';
    if (await saveKey(key, [...items, { trigger: normalized, expansion }])) {
      trigger = '';
      expansion = '';
    }
  }
  const remove = (index: number) => saveKey(key, items.filter((_, i) => i !== index));
</script>

<div class="list-editor">
  {#if items.length}
    <ul class="snippet-rows" aria-labelledby={labelledby}>
      {#each items as item, index (item.trigger)}
        <li>
          <div><span class="snippet-trigger">“{item.trigger}”</span><span class="snippet-expansion">{item.expansion}</span></div>
          <button type="button" class="icon" aria-label="{t("Remover o snippet")} {item.trigger}" {disabled} onclick={() => remove(index)}><X size={14} /></button>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="empty-line">{t("Nenhum snippet.")}</p>
  {/if}
  <form class="snippet-form" onsubmit={(e) => { e.preventDefault(); add(); }}>
    <input type="text" bind:value={trigger} placeholder={t("Gatilho (ex.: meu email)")} aria-label={t("Gatilho")} {disabled} autocomplete="off" />
    <textarea bind:value={expansion} rows="2" placeholder={t("Texto que entra no lugar")} aria-label={t("Texto")} {disabled}></textarea>
    <button type="submit" class="secondary" disabled={disabled || !trigger.trim() || !expansion.trim()}><Plus size={15} />{t("Adicionar snippet")}</button>
  </form>
  {#if error}<p class="field-error" role="alert">{t(error)}</p>{/if}
</div>
