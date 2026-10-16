<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";
  import Field from '$lib/settings/Field.svelte';
  import SettingsFrame from '$lib/settings/SettingsFrame.svelte';
</script>
<svelte:head><title>{t("Dicionário · FastWisper")}</title></svelte:head>

<!-- Conteúdo do Hub v1; o redesenho de Personalização fica para depois. -->
<h1 id="dicionario-title">{t("Dicionário")}</h1>
<SettingsFrame>
  <section class="group" aria-labelledby="dicionario-title">
    <Field key="dictionary.words" />
    <Field key="dictionary.replace" />
  </section>
</SettingsFrame>
