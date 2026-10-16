import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

/** Um quadro do stream `meter`: só o nível do microfone, nunca áudio. */
export interface MeterFrame { level: number; recording: boolean }

let listeners = 0;

/** Escuta o nível do microfone. O backend mantém um stream só, aberto
 * enquanto houver alguém escutando (cartão de estado, Microfone...). */
export async function followMeter(onFrame: (frame: MeterFrame) => void): Promise<() => void> {
  if (!isTauri()) return () => {};
  const unlisten = await listen<MeterFrame>('meter', (event) => onFrame(event.payload));
  if (listeners++ === 0) void invoke('meter_start').catch(() => {});
  let done = false;
  return () => {
    if (done) return;
    done = true;
    unlisten();
    if (--listeners === 0) void invoke('meter_stop').catch(() => {});
  };
}
