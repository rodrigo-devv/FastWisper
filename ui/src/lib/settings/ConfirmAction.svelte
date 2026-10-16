<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";
  import { onMount, tick } from 'svelte';
  let { title, description, confirmLabel = "Apagar", busy = false, onconfirm, oncancel }: {
    title:string; description:string; confirmLabel?:string; busy?:boolean; onconfirm:()=>void; oncancel:()=>void;
  } = $props();
  let dialog:HTMLDialogElement;
  onMount(() => {
    const previous = document.activeElement as HTMLElement | null;
    dialog.showModal(); dialog.querySelector<HTMLButtonElement>('.cancel')?.focus();
    return () => { dialog.close(); void tick().then(() => previous?.focus()); };
  });
</script>
<dialog bind:this={dialog} aria-labelledby="confirm-title" aria-describedby="confirm-description" oncancel={e => { e.preventDefault(); if (!busy) oncancel(); }} onkeydown={e => { e.stopPropagation(); }}>
  <h2 id="confirm-title">{title}</h2><p id="confirm-description">{description}</p>
  <div class="actions"><button type="button" class="secondary cancel" disabled={busy} onclick={oncancel}>{t("Cancelar")}</button><button type="button" class="secondary danger" disabled={busy} onclick={onconfirm}>{busy ? t('Apagando…') : t(confirmLabel)}</button></div>
</dialog>
<style>
  dialog { margin:auto; width:420px; max-width:calc(100vw - 48px); box-sizing:border-box; color:var(--text); background:var(--surface); border:1px solid var(--border); border-radius:16px; padding:28px; box-shadow:0 24px 60px rgb(0 0 0 / .67); }
  dialog::backdrop { background:rgb(0 0 0 / .65); }
  h2 { font-size:20px; } p { color:var(--text-muted); font-size:14px; line-height:1.5; }
  .actions { display:flex; gap:10px; justify-content:flex-end; margin-top:24px; } .danger { color:var(--err); }
</style>
