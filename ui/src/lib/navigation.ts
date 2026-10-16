import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { goto } from '$app/navigation';

/** Abas do modal de Configurações (as mesmas de `src-tauri/src/navigation.rs`). */
export const SETTINGS_TABS = ['geral', 'sistema', 'vibe-coding', 'experimental', 'privacidade', 'ajuda', 'sobre'] as const;
export type SettingsTab = (typeof SETTINGS_TABS)[number];
export const isSettingsTab = (tab: unknown): tab is SettingsTab => SETTINGS_TABS.includes(tab as SettingsTab);

/** `fastwisper hub <página>`: o backend só entrega destinos da própria
 * lista; uma página do hub ou uma aba de Configurações. */
type Destination = { kind: 'page'; route: string } | { kind: 'settings'; tab: string };
const internal = (route: unknown): route is string => typeof route === 'string' && /^\/[a-z/]*$/.test(route);

export async function followHubNavigation(openSettings: (tab: SettingsTab) => void): Promise<void> {
  if (!isTauri()) return;
  const go = (destination: Destination | null) => {
    if (destination?.kind === 'page' && internal(destination.route)) void goto(destination.route);
    else if (destination?.kind === 'settings' && isSettingsTab(destination.tab)) openSettings(destination.tab);
  };
  try {
    go(await invoke<Destination | null>('initial_destination'));
    await listen<Destination>('navigate', (event) => go(event.payload));
  } catch { /* Sem destino pedido: segue na página atual. */ }
}
