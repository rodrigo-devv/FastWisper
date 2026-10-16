import { source } from "$lib/i18n/index.svelte";
import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { writable } from 'svelte/store';

// DTOs fechados: nunca repassar Response/Config inteiros à webview.
export interface HubStatus { state: string | null; available: boolean }
export interface ConfigSummary { sounds: boolean; llm_enabled: boolean; store_text: boolean; store_audio: boolean }
export interface ThemeColors { background: string | null; foreground: string | null; accent: string | null; surface: string | null }
export const getStatus = () => invoke<HubStatus>('hub_status');
export const getConfigSummary = () => invoke<ConfigSummary>('config_summary');
/** Versão do FastWisper (Cargo), para o rodapé do menu. */
export const hubVersion = () => invoke<string>('hub_version');

/** A mesma validação de `HubTheme` no core; nomes nunca são caminhos. */
export type HubTheme = 'auto' | 'dark' | 'light' | `omarchy:${string}`;
export const parseTheme = (value: unknown): HubTheme | null => {
  if (value === 'auto' || value === 'dark' || value === 'light') return value;
  return typeof value === 'string' && /^omarchy:[a-zA-Z0-9_-]{1,128}$/.test(value) ? value as HubTheme : null;
};
export interface OmarchyTheme { name: string; colors: ThemeColors }
export const getOmarchyThemes = () => isTauri() ? invoke<OmarchyTheme[]>('omarchy_themes') : Promise.resolve([]);
export const getThemeColors = (name?: string) => isTauri() ? invoke<ThemeColors | null>('theme_colors', { name: name ?? null }) : Promise.resolve(null);
export const themeWarning = writable<string | null>(null);
const HEX = /^#[0-9a-fA-F]{6}$/;
const AUTO_PROPS = ['color-scheme', '--bg', '--text', '--accent', '--surface', '--surface-2', '--border', '--text-muted', '--hover', '--ok', '--warn', '--err'];
let wanted: HubTheme | null = null;
let request = 0;

/** Só tokens do hub. Nunca modifica o tema ou os arquivos do Omarchy. */
export async function applyTheme(theme: HubTheme): Promise<void> {
  wanted = theme;
  const version = ++request;
  let effective: HubTheme = theme;
  let colors: ThemeColors | null = null;
  let warning: string | null = null;
  if (theme === 'auto' || theme.startsWith('omarchy:')) {
    try { colors = await getThemeColors(theme === 'auto' ? undefined : theme.slice(8)); } catch { /* usa fallback */ }
    if (!colors && theme.startsWith('omarchy:')) {
      effective = 'auto';
      warning = source("O tema “{0}” não está disponível. Usando Automático.", theme.slice(8));
      try { colors = await getThemeColors(); } catch { /* sem Omarchy: Escuro */ }
    }
  }
  if (version !== request) return;
  const root = document.documentElement;
  for (const prop of AUTO_PROPS) root.style.removeProperty(prop);
  root.dataset.theme = effective;
  if (colors) for (const [prop, value] of Object.entries(deriveTokens(colors))) root.style.setProperty(prop, value);
  themeWarning.set(warning);
}

/** Automático: tema do Omarchy (sem ele, ficam os tokens do Escuro). */
export async function loadTheme(): Promise<void> {
  await applyTheme('auto');
}

/** Paletas fixas usadas também nas miniaturas. */
export const fixedColors: Record<'dark' | 'light', ThemeColors> = {
  dark: { background: '#1b1a19', foreground: '#ecebe6', accent: '#8b5cf6', surface: '#232221' },
  light: { background: '#F6F6F3', foreground: '#1B1B19', accent: '#08729C', surface: '#FFFFFF' },
};

/** A galeria e o hub derivam as mesmas cores, sem corrigir o contraste do tema. */
export function deriveTokens(colors: ThemeColors): Record<string, string> {
  const tokens: Record<string, string> = {};
  const names: Record<keyof ThemeColors, string> = { background: 'bg', foreground: 'text', accent: 'accent', surface: 'surface' };
  for (const [key, value] of Object.entries(colors) as [keyof ThemeColors, string | null][]) {
    if (value && HEX.test(value)) tokens[`--${names[key]}`] = value;
  }
  const background = colors.background;
  if (!background || !HEX.test(background)) return tokens;
  // Surface ausente é derivada das cores do próprio tema, não do tema anterior.
  tokens['--surface'] ??= 'color-mix(in srgb, var(--bg) 97%, var(--text))';
  const channel = (i: number) => parseInt(background.slice(i, i + 2), 16) / 255;
  const lin = (c: number) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
  const light = 0.2126 * lin(channel(1)) + 0.7152 * lin(channel(3)) + 0.0722 * lin(channel(5)) > 0.4;
  tokens['color-scheme'] = light ? 'light' : 'dark';
  tokens['--surface-2'] = 'color-mix(in srgb, var(--surface) 86%, var(--text))';
  tokens['--border'] = 'color-mix(in srgb, var(--bg) 76%, var(--text))';
  tokens['--text-muted'] = `color-mix(in srgb, var(--text) ${light ? 72 : 64}%, var(--bg))`;
  if (light) {
    tokens['--hover'] = 'rgb(0 0 0 / .04)';
    tokens['--ok'] = '#1F7A45';
    tokens['--warn'] = '#8A6200';
    tokens['--err'] = '#B83A3A';
  }
  return tokens;
}

/** Relê `hub.theme`. Config inválida mantém as cores atuais (na abertura,
 * vale o Automático). */
export async function refreshTheme(): Promise<void> {
  let theme: HubTheme | null = null;
  try { theme = parseTheme(await invoke<unknown>('hub_theme')); } catch { /* mantém */ }
  if (theme) await applyTheme(theme);
  else if (wanted === null) await applyTheme('auto');
}

/** Aplica `hub.theme` e acompanha a config e o tema do Omarchy ao vivo
 * (evento `theme-changed` do backend). Fora do Tauri (`npm run dev`), o
 * tema vem de `?tema=auto|dark|light`. */
export async function followTheme(): Promise<void> {
  if (!isTauri()) {
    await applyTheme(parseTheme(new URLSearchParams(location.search).get('tema')) ?? 'auto');
    return;
  }
  await refreshTheme();
  try { await listen('theme-changed', () => { void refreshTheme(); }); } catch { /* sem eventos: fica o tema da abertura */ }
}
