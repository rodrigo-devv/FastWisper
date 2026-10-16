<script lang="ts">
  import type { Snippet } from 'svelte';
  import WipBadge from './WipBadge.svelte';
  let { title, description = '', wip = false, fullWidth = false, titleId, descriptionId, controlId, metadata, children, error }: {
    title: string; description?: string; wip?: boolean; fullWidth?: boolean;
    titleId?: string; descriptionId?: string; controlId?: string;
    metadata?: Snippet; children?: Snippet; error?: Snippet;
  } = $props();
</script>
<div class="adjustment-row" class:full-width={fullWidth}>
  <div class="field-text">
    <div class="field-head">
      {#if controlId}<label class="field-label" for={controlId} id={titleId}>{title}</label>
      {:else}<span class="field-label" id={titleId}>{title}</span>{/if}
      {#if wip}<WipBadge />{/if}
      {@render metadata?.()}
    </div>
    {#if description}<p class="help" id={descriptionId}>{description}</p>{/if}
  </div>
  <div class="field-control">{@render children?.()}</div>
  {@render error?.()}
</div>
