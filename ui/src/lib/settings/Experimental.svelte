<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";
  import { FlaskConical } from '@lucide/svelte';
  import SettingsGroup from './SettingsGroup.svelte';
  import AdjustmentRow from './AdjustmentRow.svelte';
  import WideButton from './WideButton.svelte';
  import DevelopmentNotice from './DevelopmentNotice.svelte';
  import WipSwitchRow from './WipSwitchRow.svelte';
  import Field from './Field.svelte';
</script>
<DevelopmentNotice icon={FlaskConical}>{t("Recursos em desenvolvimento (WIP). Podem mudar ou sumir entre versões, e ficam desligados por padrão.")}</DevelopmentNotice>
<SettingsGroup>
  <Field key="experimental.notes" wip />
  <WipSwitchRow id="wip-app-context" title={t("Contexto do app")} description={t("Usa só a classe do app em foco para ajustar a formatação (terminal, e-mail, chat). Nunca lê o título nem o conteúdo da janela.")} />
  <WipSwitchRow id="wip-app-style" title={t("Estilo por app")} description={t("Um estilo de formatação diferente para cada app.")} />
  <AdjustmentRow title={t("Pílula em outras posições")} description={t("No topo, nos cantos ou junto do cursor.")} wip><WideButton disabled>{t("Em breve")}</WideButton></AdjustmentRow>
</SettingsGroup>
