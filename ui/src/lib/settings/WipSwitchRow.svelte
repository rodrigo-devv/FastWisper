<script lang="ts">
  import AdjustmentRow from './AdjustmentRow.svelte';
  // Sem chave nem escrita própria. A H3b liga a linha de notas ao store
  // passando o estado e a ação, depois de entregar o backend opt-in.
  let { id, title, description, checked = false, disabled = true, ontoggle }: {
    id: string; title: string; description: string;
    checked?: boolean; disabled?: boolean; ontoggle?: () => void;
  } = $props();
</script>
<AdjustmentRow {title} {description} wip titleId={`${id}-label`} descriptionId={`${id}-help`} controlId={id}>
  <button {id} type="button" role="switch" class="switch" aria-checked={checked} aria-labelledby={`${id}-label`} aria-describedby={`${id}-help`} {disabled} onclick={() => { if (!disabled) ontoggle?.(); }}><span></span></button>
</AdjustmentRow>
