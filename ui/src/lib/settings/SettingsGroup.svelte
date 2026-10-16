<script lang="ts">
  import type { Snippet } from 'svelte';
  let { title, id, children }: { title?: string; id?: string; children: Snippet } = $props();
</script>
<section class="settings-group" {id} aria-label={title}>
  {#if title}<h2>{title}</h2>{/if}
  <div class="settings-group-box">{@render children()}</div>
</section>
