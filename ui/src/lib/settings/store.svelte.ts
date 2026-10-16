import { t, source, setLanguage } from "$lib/i18n/index.svelte";
import { invoke } from '@tauri-apps/api/core';
import { refreshTheme } from '$lib/backend';

// Projeção do backend: só as chaves do catálogo, nunca a config inteira.
interface Snapshot { values: Record<string, unknown>; defaults: Record<string, unknown>; restart_keys: string[] }
type Outcome =
  | { status: 'applied' | 'pending'; restart_required: string[] }
  | { status: 'not_running' | 'outdated' }
  | { status: 'refused'; message: string };
export type Change = [string, unknown];
export interface Notice { tone: 'ok' | 'info' | 'warn' | 'err'; text: string }

export const settings = $state({
  loaded: false,
  error: null as string | null,
  values: {} as Record<string, unknown>,
  defaults: {} as Record<string, unknown>,
  restartKeys: [] as string[],
  /** Chaves gravadas que só valem depois de reiniciar o daemon. */
  pendingRestart: [] as string[],
  saving: {} as Record<string, boolean>,
  errors: {} as Record<string, string>,
  notice: null as Notice | null,
  restarting: false
});

let noticeTimer: ReturnType<typeof setTimeout> | undefined;
export function notify(notice: Notice, ms = 4000): void {
  settings.notice = notice;
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => { settings.notice = null; }, ms);
}

/** Mensagem do backend sem o caminho da config na frente. */
export function friendly(error: unknown): string {
  return String(error).replace(/^config inválida em .*?\.toml: /, '').replace(/^valor inválido para /, '');
}

export async function loadSettings(): Promise<void> {
  try {
    const snap = await invoke<Snapshot>('settings_get');
    settings.values = snap.values;
    setLanguage(snap.values['ui.language']);
    settings.defaults = snap.defaults;
    settings.restartKeys = snap.restart_keys;
    settings.error = null;
  } catch (error) {
    settings.error = friendly(error);
  } finally {
    settings.loaded = true;
  }
}

/** Grava as mudanças juntas (`settings_set` = `config_edit` + `reload_config`). */
export async function apply(changes: Change[]): Promise<boolean> {
  const keys = changes.map(([key]) => key);
  for (const key of keys) { settings.saving[key] = true; delete settings.errors[key]; }
  try {
    const outcome = await invoke<Outcome>('settings_set', { changes });
    for (const [key, value] of changes) settings.values[key] = value;
    if (keys.includes('ui.language')) setLanguage(settings.values['ui.language']);
    if (keys.includes('hub.theme')) void refreshTheme(); // vale na hora
    report(outcome, keys);
    return true;
  } catch (error) {
    for (const key of keys) settings.errors[key] = friendly(error);
    notify({ tone: 'err', text: source("Não foi salvo: {0}", friendly(error)) }, 6000);
    return false;
  } finally {
    for (const key of keys) settings.saving[key] = false;
  }
}

export const saveKey = (key: string, value: unknown) => apply([[key, value]]);

function report(outcome: Outcome, keys: string[]): void {
  switch (outcome.status) {
    case 'applied':
    case 'pending': {
      const restart = outcome.restart_required.filter((k) => keys.includes(k));
      for (const key of restart) if (!settings.pendingRestart.includes(key)) settings.pendingRestart.push(key);
      if (outcome.status === 'pending') notify({ tone: 'info', text: source("Salvo. Há um ditado em andamento: vale quando ele terminar.") });
      else if (restart.length === keys.length) notify({ tone: 'info', text: source("Salvo. Vale depois de reiniciar o FastWisper.") });
      else notify({ tone: 'ok', text: source("Salvo e aplicado.") }, 2500);
      break;
    }
    case 'not_running':
      notify({ tone: 'info', text: source("Salvo. O FastWisper está parado: vale no próximo início.") });
      break;
    case 'outdated':
      for (const key of keys) if (!settings.pendingRestart.includes(key)) settings.pendingRestart.push(key);
      notify({ tone: 'warn', text: source("Salvo. O FastWisper em execução é de uma versão antiga: reinicie para valer.") }, 6000);
      break;
    case 'refused':
      notify({ tone: 'err', text: source("Salvo, mas o FastWisper recusou a config: {0}", friendly(outcome.message)) }, 8000);
      break;
  }
}

export async function restartDaemon(): Promise<void> {
  settings.restarting = true;
  try {
    await invoke('daemon_restart');
    settings.pendingRestart = [];
    notify({ tone: 'ok', text: source("FastWisper reiniciado.") }, 3000);
  } catch (error) {
    notify({ tone: 'err', text: source("Não foi possível reiniciar: {0}", friendly(error)) }, 8000);
  } finally {
    settings.restarting = false;
  }
}
