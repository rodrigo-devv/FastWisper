<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";
  /** Atalho em teclas, ex.: `Alt` + `Espaço` (rótulos de `hotkeys_get`). */
  let { keys }: { keys: string[] } = $props();
</script>

<span class="keys">
  {#each keys as key, i}{#if i > 0}<span class="plus" aria-hidden="true">+</span>{/if}<kbd>{t(key)}</kbd>{/each}
</span>

<style>
  .keys { display:inline-flex; align-items:center; gap:5px; }
  .plus { color:var(--text-muted); font-size:13px; }
  kbd { font-family:var(--font-mono); font-size:11.5px; line-height:17px; color:var(--text); background:var(--surface); border:1px solid var(--border); border-bottom-width:2px; border-radius:6px; padding:2px 7px; }
</style>
