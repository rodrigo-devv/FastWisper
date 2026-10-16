import { translate, type Language } from './messages';
export type { Language } from './messages';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export const i18n = $state({ language: 'en' as Language });
export const locale = () => i18n.language;
let revision = 0;

/** Catálogos locais. Parâmetros nunca passam por tradução (texto, classes, IDs). */
export function t(key: string | null | undefined, ...values: unknown[]): string {
  return translate(i18n.language, key ?? '', values);
}
/** Mensagem em estado mantém o molde de origem, para retraduzir ao vivo. */
export function source(key: string, ...values: unknown[]): string {
  return key.replace(/\{(\d+)\}/g, (match, index) => index < values.length ? String(values[index]) : match);
}
export function setLanguage(value: unknown): void {
  if (value === 'en' || value === 'pt-BR') {
    revision += 1;
    i18n.language = value;
    if (typeof document !== 'undefined') document.documentElement.lang = value;
  }
}
export async function refreshLanguage(): Promise<void> {
  const request = ++revision;
  try {
    const language = await invoke<Language>('ui_language');
    if (request === revision) setLanguage(language);
  } catch { /* mantém idioma válido */ }
}
export async function followLanguage(): Promise<void> {
  if (!isTauri()) {
    setLanguage(new URLSearchParams(location.search).get('language') ?? 'en');
    return;
  }
  await refreshLanguage();
  try { await listen('theme-changed', () => { void refreshLanguage(); }); } catch { /* sem eventos: idioma da abertura */ } // observador da config
}
