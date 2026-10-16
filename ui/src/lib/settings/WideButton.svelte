<script lang="ts">
  import type { Snippet } from 'svelte';
  let { children, onclick, disabled = false, danger = false }: { children: Snippet; onclick?: () => void; disabled?: boolean; danger?: boolean } = $props();
</script>
<button type="button" class="wide-button" class:danger {disabled} {onclick}>{@render children()}</button>
<style>
  .wide-button { width:200px; max-width:100%; padding:10px 16px; border:1px solid transparent; border-radius:10px; background:var(--surface-2); color:var(--text); font:inherit; font-size:14px; font-weight:500; cursor:pointer; }
  .wide-button:hover:not(:disabled) { border-color:var(--text-muted); }
  .danger { color:var(--err); }
</style>
