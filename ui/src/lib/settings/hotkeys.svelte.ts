import { t } from "$lib/i18n/index.svelte";
import { invoke, isTauri } from '@tauri-apps/api/core';

// Atalhos editáveis (`[hotkey]`): os mesmos de `fastwisper config set hotkey.*`.
// Validação, conflitos e aplicação no Hyprland ficam no backend (fw_core::hotkeys).
export type Action = 'dictate' | 'hands_free' | 'paste_last';
export interface HotkeyRow { action: Action; key: string; combo: string; labels: string[]; default: string; default_labels: string[] }
export interface KeyDef { name: string; keycode: number; code: string; label: string }
export interface Conflict { description: string; combo: string }
export interface Preview { action: Action; combo: string; labels: string[]; conflicts: Conflict[]; checked: boolean; note?: string }
export interface Saved { combo: string; labels: string[]; message: string; applied: { status: 'unchanged' | 'reloaded' | 'written'; reload_error?: string } }

/** Rótulos padrão: o que o hub mostra antes de ler a config. */
const FALLBACK: Record<Action, string[]> = {
  dictate: ['Alt', "Espaço"],
  hands_free: ['Alt', 'Shift', "Espaço"],
  paste_last: ['Super', 'Alt', 'V']
};

export const hotkeys = $state({
  loaded: false,
  rows: [] as HotkeyRow[],
  keys: [] as KeyDef[]
});

export async function loadHotkeys(): Promise<void> {
  if (!isTauri()) return;
  try {
    const view = await invoke<{ rows: HotkeyRow[]; keys: KeyDef[] }>('hotkeys_get');
    hotkeys.rows = view.rows;
    hotkeys.keys = view.keys;
  } catch {
    // Sem config legível: ficam os rótulos padrão; Configurações mostra o erro.
  } finally {
    hotkeys.loaded = true;
  }
}

export function labelsOf(action: Action): string[] {
  return (hotkeys.rows.find((row) => row.action === action)?.labels ?? FALLBACK[action]).map(key => t(key));
}

export function rowOf(action: Action): HotkeyRow | undefined {
  return hotkeys.rows.find((row) => row.action === action);
}

export function updateRow(action: Action, saved: Saved): void {
  const row = rowOf(action);
  if (row) { row.combo = saved.combo; row.labels = saved.labels; }
}

const SUPER_CODES = new Set(['MetaLeft', 'MetaRight', 'OSLeft', 'OSRight']);
const MODIFIER_CODES = new Set([...SUPER_CODES, 'ControlLeft', 'ControlRight', 'AltLeft', 'AltRight', 'ShiftLeft', 'ShiftRight', 'AltGraph', 'CapsLock']);
const ORDER = ['SUPER', 'CTRL', 'ALT', 'SHIFT'];
export const isModifierCode = (code: string): boolean => MODIFIER_CODES.has(code);

/**
 * Modificadores de um evento, na ordem canônica. `held` são os códigos das
 * modificadoras seguradas: no WebKitGTK o Super nem sempre chega em
 * `metaKey`, então a tecla Super física também conta. (AltGr não é Alt.)
 */
export function modifiersOf(event: KeyboardEvent, held: Set<string> = new Set()): string[] {
  const on = new Set<string>();
  if (event.metaKey || event.getModifierState?.('Meta') || event.getModifierState?.('OS') || event.getModifierState?.('Super')) on.add('SUPER');
  if (event.ctrlKey) on.add('CTRL');
  if (event.altKey) on.add('ALT');
  if (event.shiftKey) on.add('SHIFT');
  for (const code of held) if (SUPER_CODES.has(code)) on.add('SUPER');
  return ORDER.filter((m) => on.has(m));
}

export const MODIFIER_LABELS: Record<string, string> = { SUPER: 'Super', CTRL: 'Ctrl', ALT: 'Alt', SHIFT: 'Shift' };

/** Tecla principal pela posição física (`KeyboardEvent.code`), como o Hyprland a vê. */
export function keyOf(code: string): KeyDef | undefined {
  return hotkeys.keys.find((key) => key.code === code);
}
