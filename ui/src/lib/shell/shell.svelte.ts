import { tick } from 'svelte';
import type { SettingsTab } from '$lib/navigation';

/** Estado do shell do Hub v2 (menu lateral e modal), compartilhado pelas páginas. */
export const shell = $state({
  /** Aba aberta do modal de Configurações; `null` = fechado. O modal fica
   * sobre a página atual, não é uma rota. */
  settings: null as SettingsTab | null,
  /** Seção da aba para rolar até ela ao abrir (`id` no conteúdo do modal). */
  section: null as string | null,
});

let opener: HTMLElement | null = null;

export function openSettings(tab: SettingsTab = 'geral', section: string | null = null): void {
  if (shell.settings === null && document.activeElement instanceof HTMLElement) opener = document.activeElement;
  shell.settings = tab;
  shell.section = section;
}

/** Fecha e devolve o foco a quem abriu (depois que o shell deixa de ser `inert`). */
export function closeSettings(): void {
  shell.settings = null;
  shell.section = null;
  const back = opener;
  opener = null;
  void tick().then(() => back?.focus());
}
