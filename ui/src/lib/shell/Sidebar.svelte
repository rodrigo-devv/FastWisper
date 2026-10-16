<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { isTauri } from '@tauri-apps/api/core';
  import {
    AudioLines, BookOpenText, Brain, ChevronDown, History, LayoutGrid, NotebookPen, Palette, Scissors, Settings, ShieldCheck, Sparkles,
  } from '@lucide/svelte';
  import { hubVersion } from '$lib/backend';
  import NavItem from './NavItem.svelte';
  import StatusCard from './StatusCard.svelte';
  import { shell, openSettings } from './shell.svelte';

  // Menu lateral do Hub v2.
  const path = $derived(page.url.pathname.replace(/\/$/, '') || '/');
  const personal = $derived([
    { href: '/personalizacao/dicionario', label: t("Dicionário"), icon: BookOpenText },
    { href: '/personalizacao/snippets', label: t("Snippets"), icon: Scissors },
    { href: '/personalizacao/estilo', label: t("Estilo"), icon: Sparkles },
  ]);
  let open = $state(true);
  let version = $state('');

  onMount(() => {
    if (isTauri()) void hubVersion().then((v) => { version = v; }).catch(() => {});
  });
</script>

<aside class="sidebar">
  <a href="/" class="brand" aria-label={t("FastWisper: Início")}><AudioLines size={22} strokeWidth={1.8} aria-hidden="true" />FastWisper</a>
  <nav aria-label={t("Navegação principal")}>
    <NavItem href="/" label={t("Início")} icon={LayoutGrid} active={path === '/'} />
    <NavItem label={t("Personalização")} icon={Brain} expanded={open} controls="menu-personalizacao" onclick={() => (open = !open)}>
      {#snippet trailing()}<ChevronDown size={15} class="chevron {open ? 'open' : ''}" aria-hidden="true" />{/snippet}
    </NavItem>
    {#if open}
      <div id="menu-personalizacao" class="sub">
        {#each personal as item}<NavItem {...item} indent active={path === item.href} />{/each}
      </div>
    {/if}
    <NavItem href="/notas" label={t("Notas")} icon={NotebookPen} active={path === '/notas'} />
    <NavItem href="/historico" label={t("Histórico")} icon={History} active={path === '/historico'} />
  </nav>

  <div class="space"></div>
  <StatusCard />
  <hr />

  <div class="bottom">
    <NavItem href="/temas" label={t("Temas")} icon={Palette} active={path === '/temas'} />
    <NavItem label={t("Configurações")} icon={Settings} active={shell.settings !== null} onclick={() => openSettings('geral')} />
  </div>
  <footer>
    <span>FastWisper{version ? ` v${version}` : ''} {t("· local")}</span>
    <ShieldCheck size={15} aria-label={t("Tudo roda nesta máquina")} />
  </footer>
</aside>

<style>
  .sidebar { line-height:normal; width:248px; box-sizing:border-box; height:100dvh; overflow-y:auto; flex:none; display:flex; flex-direction:column; padding:22px 12px 14px; background:var(--surface); border-right:1px solid var(--border); }
  .brand { display:flex; align-items:center; gap:10px; padding:0 10px; font-size:18px; font-weight:600; letter-spacing:-.5px; color:var(--text); align-self:flex-start; border-radius:8px; }
  .brand :global(svg) { color:var(--accent); }
  nav { display:flex; flex-direction:column; gap:2px; padding-top:26px; }
  .sub { display:flex; flex-direction:column; gap:2px; }
  :global(.chevron) { transition:transform .15s ease; transform:rotate(-90deg); }
  :global(.chevron.open) { transform:none; }
  .space { flex:1 1 0; min-height:8px; }
  hr { border:0; height:1px; background:var(--border); margin:14px 6px; flex:none; }
  .bottom { display:flex; flex-direction:column; gap:2px; }
  footer { display:flex; align-items:center; justify-content:space-between; padding:12px 10px 0; font-size:12px; color:var(--text-muted); }
  @media (prefers-reduced-motion: reduce) { :global(.chevron) { transition:none; } }
</style>
