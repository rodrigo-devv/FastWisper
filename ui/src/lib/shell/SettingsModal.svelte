<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";
  import { tick, onMount } from 'svelte';
  import { FlaskConical, Hash, Laptop, ShieldCheck, SlidersHorizontal, Cloud, CloudOff, CircleHelp, Info, X } from '@lucide/svelte';
  import { SETTINGS_TABS, type SettingsTab } from '$lib/navigation';
  import SettingsFrame from '$lib/settings/SettingsFrame.svelte';
  import Geral from '$lib/settings/Geral.svelte';
  import Sistema from '$lib/settings/Sistema.svelte';
  import Privacidade from '$lib/settings/Privacidade.svelte';
  import VibeCoding from '$lib/settings/VibeCoding.svelte';
  import Experimental from '$lib/settings/Experimental.svelte';
  import Ajuda from '$lib/settings/Ajuda.svelte';
  import Sobre from '$lib/settings/Sobre.svelte';
  import { settings } from '$lib/settings/store.svelte';
  import { hubVersion } from '$lib/backend';
  import { shell, openSettings, closeSettings } from './shell.svelte';
  import NavItem from './NavItem.svelte';

  const TABS: Record<SettingsTab, { label: string; icon: typeof X }> = $derived({
    geral: { label: t("Geral"), icon: SlidersHorizontal },
    sistema: { label: t("Sistema"), icon: Laptop },
    'vibe-coding': { label: t("Vibe coding"), icon: Hash },
    experimental: { label: 'Experimental', icon: FlaskConical },
    privacidade: { label: t("Dados e privacidade"), icon: ShieldCheck },
    ajuda: { label: t("Ajuda"), icon: CircleHelp },
    sobre: { label: t("Sobre"), icon: Info },
  });
  let dialog = $state<HTMLElement>();
  const tab = $derived(shell.settings);
  let version = $state('…'), scroll = $state<HTMLElement>(), inner = $state<HTMLElement>(), fade = $state(false);
  const online = $derived(settings.values['llm.ollama_cloud'] === true || (
    settings.values['llm.enabled'] === true && settings.values['llm.engine'] === 'openai_compatible' &&
    settings.values['llm.remote_consent'] === true && !/^https?:\/\/(127\.\d{1,3}\.\d{1,3}\.\d{1,3}|\[::1\])(:\d+)?(\/|$)/.test(String(settings.values['llm.endpoint'] ?? ''))
  ));
  onMount(() => { void hubVersion().then(v => version = v).catch(() => version = '—'); });
  function updateFade() { fade = !!scroll && scroll.scrollHeight - scroll.clientHeight - scroll.scrollTop > 2; }
  $effect(() => {
    if (!inner || !scroll) return;
    const observer = new ResizeObserver(updateFade); observer.observe(inner); observer.observe(scroll);
    updateFade(); return () => observer.disconnect();
  });
  $effect(() => { if (tab) void tick().then(() => { scroll?.scrollTo({top:0}); updateFade(); }); });

  $effect(() => {
    if (tab && dialog && !dialog.contains(document.activeElement)) void tick().then(() => dialog?.focus());
  });
  // Abriu numa seção (ex.: Ajuda → Sistema › Conferência): rola até ela.
  $effect(() => {
    const id = shell.section;
    if (tab && id && settings.loaded) void tick().then(() => dialog?.querySelector(`#${CSS.escape(id)}`)?.scrollIntoView({ block: 'start' }));
  });

  function focusables(): HTMLElement[] {
    return [...(dialog?.querySelectorAll<HTMLElement>('a[href], button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])') ?? [])]
      .filter((el) => el.offsetParent !== null);
  }

  function onkeydown(event: KeyboardEvent): void {
    if (!tab || event.defaultPrevented) return;
    if (event.key === 'Escape') { event.preventDefault(); closeSettings(); return; }
    if (event.key !== 'Tab') return;
    const list = focusables();
    if (!list.length) { event.preventDefault(); return; }
    const first = list[0], last = list[list.length - 1];
    const inside = dialog?.contains(document.activeElement);
    if (event.shiftKey && (document.activeElement === first || !inside)) { event.preventDefault(); last.focus(); }
    else if (!event.shiftKey && (document.activeElement === last || !inside)) { event.preventDefault(); first.focus(); }
  }
</script>

<svelte:window {onkeydown} />

{#if tab}
  <!-- O clique no véu fecha; pelo teclado, Esc (acima). -->
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="veil" onclick={(e) => { if (e.target === e.currentTarget) closeSettings(); }}>
    <div class="modal" role="dialog" aria-modal="true" aria-labelledby="settings-title" tabindex="-1" bind:this={dialog}>
      <nav class="modal-menu" aria-label={t("Abas de Configurações")}>
        <p class="section-label">{t("CONFIGURAÇÕES")}</p>
        {#each SETTINGS_TABS.filter(id => !['privacidade', 'ajuda', 'sobre'].includes(id)) as id}
          {@const item = TABS[id]}
          <NavItem label={item.label} icon={item.icon} active={tab === id} onclick={() => openSettings(id)} />
        {/each}
        <div class="divider"></div><p class="section-label">{t("DADOS")}</p>
        <NavItem label={TABS.privacidade.label} icon={ShieldCheck} active={tab === 'privacidade'} onclick={() => openSettings('privacidade')} />
        <!-- Ajuda e Sobre não são dados: divisor sem rótulo (A-12). -->
        <div class="divider plain"></div>
        <NavItem label={t("Ajuda")} icon={CircleHelp} active={tab === 'ajuda'} onclick={() => openSettings('ajuda')} />
        <NavItem label={t("Sobre")} icon={Info} active={tab === 'sobre'} onclick={() => openSettings('sobre')} />
        <footer><span>{t("FastWisper v")}{version}</span>{#if online}<Cloud size={16} aria-label={t("Nuvem online")} />{:else}<CloudOff size={16} aria-label="Offline" />{/if}</footer>
      </nav>
      <div class="content-area">
        <div class="content settings-modal" bind:this={scroll} onscroll={updateFade}>
          <div bind:this={inner}>
            <header>
              <h1 id="settings-title">{TABS[tab].label}</h1>
              <button type="button" class="close" aria-label={t("Fechar")} onclick={closeSettings}><X size={18} /></button>
            </header>
            {#if tab === 'sobre'}<Sobre {version} />
            {:else if tab === 'ajuda'}<Ajuda />
            {:else}<SettingsFrame modal>
              {#key tab}
                {#if tab === 'geral'}<Geral />
                {:else if tab === 'sistema'}<Sistema />
                {:else if tab === 'vibe-coding'}<VibeCoding />
                {:else if tab === 'experimental'}<Experimental />
                {:else}<Privacidade />{/if}
              {/key}
            </SettingsFrame>{/if}
          </div>
        </div>
        {#if fade}<div class="scroll-fade" aria-hidden="true"></div>{/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .veil { position:fixed; inset:0; z-index:20; display:grid; place-items:center; padding:16px; background:rgb(0 0 0 / .65); }
  .modal { width:min(1000px, 100%); height:min(648px, 100%); box-sizing:border-box; display:flex; background:var(--bg); border:1px solid var(--border); border-radius:16px; box-shadow:0 24px 60px rgb(0 0 0 / .67); overflow:hidden; }
  .modal:focus { outline:none; }
  .modal-menu { width:240px; flex:none; box-sizing:border-box; background:var(--surface); border-right:1px solid var(--border); padding:10px 12px 14px; display:flex; flex-direction:column; }
  .section-label { font-size:11.5px; font-weight:600; letter-spacing:1.2px; color:var(--text-muted); margin:0; padding:18px 10px 8px; }
  .divider { height:1px; background:var(--border); margin:14px 10px 0; }
  .divider.plain { margin:10px 10px 8px; }
  footer { margin-top:auto; padding:0 10px; display:flex; align-items:center; justify-content:space-between; color:var(--text-muted); font-size:12.5px; }
  .content-area { flex:1; min-width:0; position:relative; }
  .content { height:100%; overflow-y:auto; padding:40px 40px 32px; box-sizing:border-box; scrollbar-width:thin; scrollbar-color:var(--border) transparent; }
  header { display:flex; align-items:center; justify-content:space-between; gap:16px; }
  h1 { font-size:24px; font-weight:500; letter-spacing:-.3px; }
  .close { display:inline-flex; padding:6px; border:0; border-radius:8px; background:none; color:var(--text-muted); cursor:pointer; }
  .close:hover { background:var(--hover); color:var(--text); }
  .scroll-fade { position:absolute; bottom:0; left:0; right:0; height:90px; background:linear-gradient(transparent,var(--bg)); pointer-events:none; }
  @media(max-width:760px) {
    .modal { flex-direction:column; }
    .modal-menu { width:100%; border-right:0; border-bottom:1px solid var(--border); flex-direction:row; flex-wrap:wrap; gap:4px; padding:10px; }
    .modal-menu :global(.nav-item) { width:auto; font-size:13px; padding:8px; }
    .section-label, .divider, footer { display:none; }
    .content-area { min-height:0; }
    .content { padding:24px 18px; }
  }
</style>
