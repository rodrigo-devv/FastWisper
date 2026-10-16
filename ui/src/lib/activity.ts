import { t, locale } from "$lib/i18n/index.svelte";
import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

// DTOs fechados do Início e do Histórico (src-tauri/src/home.rs e history.rs):
// estado e métricas, nunca texto ditado nem título de janela.
export type StateName = 'idle' | 'recording' | 'transcribing';
export type Readiness = 'loading' | 'ready' | 'failed';
export type VramMode = 'automatico' | 'desempenho' | 'sob_demanda' | 'jogo';
export type Profile = 'desempenho' | 'sob_demanda' | 'jogo';
export type ModelState = 'unloaded' | 'loading' | 'loaded' | 'failed' | 'external' | 'disabled';

export interface VramView {
  mode: VramMode; profile: Profile; game: boolean; asr: ModelState; llm: ModelState;
  vram_mb: number | null; unload_pending: boolean;
}
export interface LastView {
  outcome: string; failed: boolean; message: string | null; language: string | null;
  words: number | null; audio_ms: number; speech_ms: number | null; asr_ms: number | null;
  llm: string | null; llm_ms: number | null; e2e_ms: number | null; load_ms: number | null;
  llm_load_ms: number | null; profile: Profile | null; delivery: string | null; delivery_reason: string | null;
}
export interface DaemonView {
  available: boolean; state: StateName | null; hands_free: boolean; note_session: boolean; asr: Readiness | null;
  vram: VramView | null; last: LastView | null;
}
export interface Notice { item: string; level: 'warn' | 'fail'; detail: string; hint: string | null }
export interface DoctorSummary { verdict: string; ok: number; notices: Notice[] }
export interface Count { name: string; count: number }
export interface Latency { count: number; p50: number; p95: number; max: number }
export interface AppRow { app: string; pasted: number; copied: number; failed: number; total: number }
export interface HistoryView {
  consecutive_weeks: number; words: number; ppm: number;
  days: number; empty: boolean; total: number; outcomes: Count[]; deliveries: Count[]; profiles: Count[];
  apps: AppRow[]; asr_ms: Latency; llm_ms: Latency; e2e_ms: Latency; load_ms: Latency; llm_load_ms: Latency;
}

export const OFFLINE: DaemonView = { available: false, state: null, hands_free: false, note_session: false, asr: null, vram: null, last: null };

/** Estado atual e cada mudança (um `subscribe` só, aberto pelo backend). */
export async function watchDaemon(onView: (view: DaemonView) => void): Promise<UnlistenFn> {
  if (!isTauri()) { onView(OFFLINE); return () => {}; }
  let live = false;
  const unlisten = await listen<DaemonView>('daemon-status', (event) => { live = true; onView(event.payload); });
  const current = await invoke<DaemonView>('daemon_view');
  if (!live) onView(current); // um evento que chegou antes é mais novo
  return unlisten;
}
export const releaseVram = () => invoke<VramView | null>('release_vram');
export const doctorSummary = () => invoke<DoctorSummary>('doctor_summary');
export const historyStats = (days: number) => invoke<HistoryView>('history_stats', { days });

// Rótulos pt-BR dos códigos do daemon.
export const MODES: { id: VramMode; label: string; help: string }[] = [
  { id: 'automatico', label: "Automático", help: "Sob demanda; vira Jogo quando um jogo abre." },
  { id: 'desempenho', label: "Desempenho", help: "Modelos sempre na VRAM: ditado mais rápido." },
  { id: 'sob_demanda', label: "Sob demanda", help: "Carrega ao ditar e libera depois de um tempo ocioso." },
  { id: 'jogo', label: "Jogo", help: "Carrega ao ditar e libera logo em seguida." },
];
export const PROFILE: Record<string, string> = { desempenho: "Desempenho", sob_demanda: "Sob demanda", jogo: "Jogo", '-': "Sem registro" };
export const MODEL_STATE: Record<ModelState, string> = {
  unloaded: "Fora da VRAM", loading: "Carregando", loaded: "Na VRAM", failed: "Falhou", external: "Externo", disabled: "Desligado",
};
export const OUTCOME: Record<string, string> = {
  transcribed: "Transcrito", no_speech: "Sem fala", too_short: "Curto demais",
  device_not_configured: "Microfone não configurado", device_unavailable: "Microfone desconectado",
  device_ambiguous: "Microfone ambíguo", device_lost: "Microfone caiu", recording_too_long: "Gravação longa demais",
  mic_silent: "Microfone mudo", capture_failed: "Falha na captura", asr_unavailable: "Modelo indisponível", asr_failed: "Falha na transcrição",
};
export const DELIVERY: Record<string, string> = { pasted: "Colado", submitted: "Colado e enviado", copied: "Copiado", failed: "Não entregue", '-': "Sem entrega" };
export const COPY_REASON: Record<string, string> = {
  requested: 'pedido', focus_changed: "o foco mudou", no_target: "sem janela com foco", session_locked: "tela bloqueada",
  app_not_approved: "app só copia", shortcut_failed: "o atalho falhou",
};
export const LLM: Record<string, string> = {
  used: "IA aplicada", disabled: "IA desligada", skipped_short: "Curto: só regras", skipped_long: "Longo: só regras", skipped_snippet: 'Snippet',
  skipped_literal: "Terminal: literal", timeout: "IA passou do prazo", rejected: "IA barrada pela guarda",
  unavailable: "IA indisponível", busy: "IA ocupada", not_loaded: "IA ainda carregando", skipped_game: "Jogo: só regras",
};
export const label = (map: Record<string, string>, key: string | null | undefined) => (key ? t(map[key] ?? key) : '—');
/** "Copiado (o foco mudou)". */
export function deliveryLabel(delivery: string | null, reason: string | null): string {
  if (!delivery) return '—';
  const [kind, why] = delivery.includes(':') ? delivery.split(':', 2) : [delivery, reason];
  const text = label(DELIVERY, kind);
  return why ? `${text} (${label(COPY_REASON, why)})` : text;
}
export const ms = (value: number | null | undefined) =>
  value == null ? '—' : value >= 10000 ? t("{0} s", (value / 1000).toLocaleString(locale(), { minimumFractionDigits: 1, maximumFractionDigits: 1 })) : t("{0} ms", value.toLocaleString(locale()));
/** Aplica no daemon e salva em `[performance] vram`, como `fastwisper vram <modo>`. */
export const setVramMode = (mode: VramMode) => invoke<string>('set_vram_mode', { mode });
