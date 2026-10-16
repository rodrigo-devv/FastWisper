<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";
  import { onMount } from 'svelte';
  import { getOmarchyThemes } from '$lib/backend';
  import { settings, saveKey } from './store.svelte';
  // O atalho para a galeria só aparece com temas do Omarchy instalados.
  let omarchyThemes = $state(false);
  onMount(() => { getOmarchyThemes().then((list) => { omarchyThemes = list.length > 0; }).catch(() => {}); });
  const themes = $derived([{ value:'auto', label:t("Automático") }, { value:'dark', label:t("Escuro") }, { value:'light', label:t("Claro") }]);
</script>
<div class="theme-cards" role="radiogroup" aria-label={t("Tema")}>
  {#each themes as theme}
    <label class="theme-card" class:chosen={settings.values['hub.theme'] === theme.value}>
      <span class="preview {theme.value}" aria-hidden="true">
        {#each theme.value === 'auto' ? ['dark', 'light'] : [theme.value] as palette}
          <span class="mini {palette}"><span class="mini-menu"><i></i><i></i><i></i></span><span class="mini-body"><i></i><i></i><i></i><b></b></span></span>
        {/each}
      </span>
      <span class="theme-label"><input type="radio" name="hub-theme" value={theme.value} checked={settings.values['hub.theme'] === theme.value} disabled={settings.saving['hub.theme']} onchange={async (e) => { const group = e.currentTarget.closest('.theme-cards'); if (!await saveKey('hub.theme', theme.value)) group?.querySelectorAll<HTMLInputElement>('input').forEach(input => input.checked = settings.values['hub.theme'] === input.value); }} />{theme.label}</span>
    </label>
  {/each}
</div>
{#if String(settings.values['hub.theme'] ?? '').startsWith('omarchy:')}
  <p class="omarchy-choice">{t("Tema do Omarchy selecionado:")} <strong>{String(settings.values['hub.theme']).slice(8)}</strong>. <a href="/temas">{t("Abrir galeria de temas")}</a></p>
{:else if omarchyThemes}
  <p class="omarchy-choice"><a href="/temas">{t("Ver temas do Omarchy")}</a></p>
{/if}
<style>
  .omarchy-choice { color:var(--text-muted); font-size:13px; margin:12px 0 0; line-height:1.5; }
  .omarchy-choice strong { color:var(--text); font-weight:500; }
  .omarchy-choice a { color:var(--accent); text-decoration:underline; text-underline-offset:3px; }
  .theme-cards { display:grid; grid-template-columns:repeat(3,minmax(0,1fr)); gap:16px; width:100%; }
  .theme-card { min-width:0; cursor:pointer; }
  .preview { display:flex; height:96px; border:1px solid var(--border); border-radius:10px; overflow:hidden; }
  .chosen .preview { outline:2px solid var(--accent); outline-offset:-2px; }
  .mini { flex:1; display:flex; min-width:0; background:#08090a; }
  .mini.light { background:#f6f6f3; }
  .mini-menu { background:#101214; width:30%; padding:10px 8px; display:flex; flex-direction:column; gap:5px; }
  .light .mini-menu { background:#fff; }
  i { display:block; height:5px; background:#303132; border-radius:3px; }
  .light i { background:#ddddd6; }
  .mini-menu i:first-child { background:#61d6ff; height:7px; }
  .light .mini-menu i:first-child { background:#0b87b8; }
  .mini-body { flex:1; padding:12px 10px; display:flex; flex-direction:column; gap:6px; min-width:0; }
  .mini-body i:first-child { background:#f1f1ec; width:60%; height:8px; }
  .light .mini-body i:first-child { background:#1b1b19; }
  b { height:22px; border:1px solid #303132; border-radius:5px; margin-top:2px; }
  .light b { background:white; border-color:#ddddd6; }
  .theme-label { display:flex; align-items:center; gap:8px; margin-top:10px; font-size:14px; line-height:1.3; }
  input { appearance:none; margin:0; width:14px; height:14px; border:1.5px solid var(--text-muted); border-radius:50%; background:none; }
  input:checked { background:var(--accent); border-color:var(--accent); }
  .theme-card:focus-within .preview { box-shadow:0 0 0 2px var(--accent-soft); }
  @media(max-width:640px) { .theme-cards { gap:10px; } .mini-menu { padding:8px 4px; } .mini-body { padding:10px 4px; } }
</style>
