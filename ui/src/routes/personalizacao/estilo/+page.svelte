<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";
  import Field from '$lib/settings/Field.svelte';
  import SettingsFrame from '$lib/settings/SettingsFrame.svelte';
</script>
<svelte:head><title>{t("Estilo · FastWisper")}</title></svelte:head>

<!-- Conteúdo do Hub v1; o redesenho de Personalização fica para depois. -->
<h1 id="estilo-title">{t("Estilo")}</h1>
<SettingsFrame>
  <section class="group" aria-labelledby="estilo-title">
    <Field key="llm.style" />
  </section>
</SettingsFrame>
