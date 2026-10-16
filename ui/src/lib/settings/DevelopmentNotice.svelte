<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { Hash } from '@lucide/svelte';
  let { icon: Icon, children }: { icon: typeof Hash; children: Snippet } = $props();
</script>
<div class="development-notice">
  <Icon size={18} aria-hidden="true" />
  <p>{@render children()}</p>
</div>
<style>
  .development-notice { display:flex; align-items:center; gap:10px; margin-top:28px; padding:12px 16px; border:1px solid var(--warn-border); border-radius:12px; background:var(--surface); }
  .development-notice :global(svg) { flex:none; color:var(--warn); }
  p { margin:0; font-size:13.5px; line-height:20px; }
</style>
