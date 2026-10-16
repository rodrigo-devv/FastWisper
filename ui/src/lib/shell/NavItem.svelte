<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { Icon as LucideIcon } from '@lucide/svelte';

  /** Item do menu lateral (e do menu do modal de Configurações, na H1):
   * link quando tem `href`, botão quando tem `onclick`. */
  let {
    label, icon: Icon, href, onclick, active = false, indent = false, disabled = false, hint, expanded, controls, trailing,
  }: {
    label: string;
    icon: typeof LucideIcon;
    href?: string;
    onclick?: () => void;
    active?: boolean;
    indent?: boolean;
    disabled?: boolean;
    /** Dica (tooltip), por exemplo de um item ainda sem destino. */
    hint?: string;
    expanded?: boolean;
    controls?: string;
    trailing?: Snippet;
  } = $props();
</script>

{#if href && !disabled}
  <a class="nav-item" class:active class:indent {href} title={hint} aria-current={active ? 'page' : undefined}>
    <Icon size={19} strokeWidth={1.7} class="nav-icon" aria-hidden="true" /><span>{label}</span>{@render trailing?.()}
  </a>
{:else}
  <button type="button" class="nav-item" class:active class:indent title={hint} aria-disabled={disabled || undefined}
    aria-expanded={expanded} aria-controls={controls} aria-current={active ? 'page' : undefined}
    onclick={() => { if (!disabled) onclick?.(); }}>
    <Icon size={19} strokeWidth={1.7} class="nav-icon" aria-hidden="true" /><span>{label}</span>{@render trailing?.()}
  </button>
{/if}

<style>
  .nav-item { display:flex; align-items:center; gap:12px; width:100%; box-sizing:border-box; padding:9px 10px; border:0; border-radius:10px; background:none; font:inherit; font-size:15px; line-height:normal; color:var(--text-muted); text-align:left; cursor:pointer; white-space:nowrap; }
  .nav-item :global(.nav-icon) { flex:none; }
  .nav-item:hover { background:var(--hover); color:var(--text); }
  .nav-item.active { background:var(--surface-2); color:var(--text); }
  .nav-item.active :global(.nav-icon) { color:var(--accent); }
  .nav-item.indent { padding-left:34px; }
  .nav-item[aria-disabled=true] { opacity:.55; cursor:default; }
  .nav-item[aria-disabled=true]:hover { background:none; color:var(--text-muted); }
  .nav-item:focus-visible { outline:2px solid var(--accent); outline-offset:-2px; }
</style>
