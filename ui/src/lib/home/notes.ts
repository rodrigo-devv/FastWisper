import { t, locale } from "$lib/i18n/index.svelte";
import { invoke } from '@tauri-apps/api/core';
export interface Note { id: number; ts: number; text: string; language: string; words: number }
export const listNotes = () => invoke<Note[]>('notes_list');
export const copyNote = (id: number) => invoke<void>('notes_copy', {id});
export const deleteNote = (id: number) => invoke<void>('notes_delete', {id});
const weekday = ['Dom', 'Seg', 'Ter', 'Qua', 'Qui', 'Sex', "Sáb"];
const month = ['jan', 'fev', 'mar', 'abr', 'mai', 'jun', 'jul', 'ago', 'set', 'out', 'nov', 'dez'];
/** Rótulo da referência: "Hoje 14:32", "Ontem" ou "Seg 13 out" (dia local). */
export function noteTime(ts: number, now = new Date()): string {
  const at = new Date(ts * 1000);
  const day = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((day(now) - day(at)) / 86_400_000);
  if (days === 0) return t("Hoje {0}:{1}", String(at.getHours()).padStart(2, '0'), String(at.getMinutes()).padStart(2, '0'));
  if (days === 1) return t('Ontem');
  return locale() === 'pt-BR' ? `${weekday[at.getDay()]} ${at.getDate()} ${month[at.getMonth()]}`
    : at.toLocaleDateString('en', { weekday: 'short', day: 'numeric', month: 'short' });
}
