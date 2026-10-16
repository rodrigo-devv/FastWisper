<script lang="ts">
  import { t, source } from "$lib/i18n/index.svelte";
  import { invoke } from '@tauri-apps/api/core';
  import Field from './Field.svelte';
  import SettingsGroup from './SettingsGroup.svelte';
  import HotkeyRow from './HotkeyRow.svelte';
  import Microphone from './Microphone.svelte';
  import { settings, notify, friendly } from './store.svelte';
  let testing = $state(false);
  async function testSound() {
    testing = true;
    try { await invoke('sound_test', {volume:Number(settings.values['audio.sound_volume'] ?? .4)}); }
    catch(e) { notify({tone:'err',text:source("O som não tocou: {0}", friendly(e))},6000); }
    finally { testing = false; }
  }
</script>
<SettingsGroup title={t("Aparência")} id="aparencia"><Field key="ui.language" /><Field key="hub.theme" help={t("Automático segue o tema do Omarchy (claro ou escuro) e as cores de destaque dele.")} /></SettingsGroup>
<SettingsGroup title={t("Ditado")}>
  <HotkeyRow key="hotkey.dictate" />
  <HotkeyRow key="hotkey.hands_free" />
  <HotkeyRow key="hotkey.paste_last" />
  <Microphone />
  <Field key="asr.languages" />
  <Field key="hotkey.max_recording_secs" />
</SettingsGroup>
<SettingsGroup title={t("Sons e pílula")}>
  <Field key="audio.sounds" />
  <Field key="audio.sound_volume" disabled={settings.values['audio.sounds'] !== true}>
    <button type="button" class="secondary" disabled={testing || settings.values['audio.sounds'] !== true} onclick={testSound}>{testing ? t('Tocando…') : t("Testar som")}</button>
  </Field>
  <Field key="overlay.enabled" />
</SettingsGroup>
