<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";
  import { NotebookPen } from '@lucide/svelte';
</script>

<svelte:head><title>{t("Notas · FastWisper")}</title></svelte:head>
<h1>{t("Notas")}</h1>
<!-- Notas por voz: recurso experimental, sem backend nesta fase. -->
<section class="empty" aria-labelledby="notas-vazio">
  <NotebookPen size={22} aria-hidden="true" />
  <h2 id="notas-vazio">{t("Em construção")}</h2>
  <p>{t("Notas por voz ainda não chegaram. Quando chegarem, ficam só nesta máquina, e o áudio nunca é guardado.")}</p>
</section>
