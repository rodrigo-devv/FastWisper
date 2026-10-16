<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";
  import ListEditor from './ListEditor.svelte';
  import { saveKey } from './store.svelte';
  // Uma lista por idioma da tabela (`inject.send_now_phrases`); grava a tabela inteira.
  const LANGUAGES = $derived([{ code: 'pt', label: t("Português"), example: '' }, { code: 'en', label: t("Inglês"), example: t("ex.: ship it") }]);
  let { key, phrases, placeholder = '', labelledby, disabled = false }: { key: string; phrases: Record<string, string[]>; placeholder?: string; labelledby: string; disabled?: boolean } = $props();
  const items = (code: string) => (Array.isArray(phrases[code]) ? phrases[code] : []);
  const save = (code: string) => (next: string[]) => saveKey(key, { ...$state.snapshot(phrases), [code]: next });
</script>

<div class="phrases-editor" role="group" aria-labelledby={labelledby}>
  {#each LANGUAGES as language (language.code)}
    <div class="language">
      <span class="language-label" id="{labelledby}-{language.code}">{language.label} ({language.code})</span>
      <ListEditor {key} items={items(language.code)} placeholder={language.example || placeholder} labelledby="{labelledby}-{language.code}" {disabled} addLabel="{t("Adicionar frase em")} {language.label.toLowerCase()}" save={save(language.code)} />
    </div>
  {/each}
</div>

<style>
  .phrases-editor { display:grid; gap:18px; width:100%; }
  .language { display:grid; gap:8px; }
  .language-label { font-size:13px; font-weight:600; color:var(--text-muted); }
</style>
