<script lang="ts">
  import { t, source } from "$lib/i18n/index.svelte";
  import { invoke } from '@tauri-apps/api/core';
  import Kbd from '$lib/shell/Kbd.svelte';
  import AdjustmentRow from './AdjustmentRow.svelte';
  import { fieldDef } from './fields';
  import { notify, friendly } from './store.svelte';
  import { MODIFIER_LABELS, isModifierCode, keyOf, labelsOf, modifiersOf, rowOf, updateRow, type Action, type Preview, type Saved } from './hotkeys.svelte';

  // Linha editável de Configurações › Geral › Ditado: Mudar → captura →
  // prévia (com aviso de conflito) → Salvar; Restaurar padrão por linha.
  let { key }: { key: string } = $props();
  const def = $derived(fieldDef(key));
  const action = $derived(key.replace('hotkey.', '') as Action);
  const row = $derived(rowOf(action));
  const current = $derived(labelsOf(action));
  const isDefault = $derived(!row || row.combo.toUpperCase().replace(/\s+/g, '') === row.default);

  type Mode = 'idle' | 'capture' | 'preview' | 'saving';
  let mode = $state<Mode>('idle');
  let held = $state<string[]>([]);
  let problem = $state('');
  let preview = $state<Preview | null>(null);
  const ids = $derived({ title: `${action}-title`, help: `${action}-help`, status: `${action}-status` });

  // Modificadoras físicas seguradas durante a captura (ver modifiersOf).
  const down = new Set<string>();
  function reset(): void { mode = 'idle'; held = []; problem = ''; preview = null; down.clear(); }

  async function check(combo: string): Promise<void> {
    try {
      preview = await invoke<Preview>('hotkey_preview', { action, combo });
      problem = '';
      mode = 'preview';
    } catch (e) {
      problem = friendly(e);
    }
  }

  function startCapture(): void { reset(); mode = 'capture'; }
  // A janela perde o foco no meio da captura: as soltas não chegam aqui.
  function onBlur(): void { down.clear(); held = []; }

  // Captura no início da propagação: nada chega ao resto do hub (o Esc não
  // fecha o modal; Enter não aperta botões).
  function onKeydown(event: KeyboardEvent): void {
    if (mode !== 'capture') return;
    event.preventDefault();
    event.stopImmediatePropagation();
    if (isModifierCode(event.code)) down.add(event.code);
    if (event.repeat) return;
    const mods = modifiersOf(event, down);
    if (event.key === 'Escape' && mods.length === 0) { reset(); return; }
    if (isModifierCode(event.code)) { held = mods; return; }
    const pressed = keyOf(event.code);
    if (!pressed) { held = mods; problem = source("Essa tecla não pode ser usada em atalho. Tente letras, números, F1–F12, Espaço ou Enter."); return; }
    held = [];
    void check([...mods, pressed.name].join('+'));
  }
  function onKeyup(event: KeyboardEvent): void {
    if (mode !== 'capture') return;
    event.preventDefault();
    event.stopImmediatePropagation();
    down.delete(event.code);
    held = modifiersOf(event, down);
  }
  $effect(() => {
    if (mode !== 'capture') return;
    window.addEventListener('keydown', onKeydown, true);
    window.addEventListener('keyup', onKeyup, true);
    window.addEventListener('blur', onBlur);
    return () => { window.removeEventListener('keydown', onKeydown, true); window.removeEventListener('keyup', onKeyup, true); window.removeEventListener('blur', onBlur); };
  });

  async function save(): Promise<void> {
    if (!preview) return;
    mode = 'saving';
    try {
      const saved = await invoke<Saved>('hotkey_save', { action, combo: preview.combo, confirm: preview.conflicts.length > 0 });
      updateRow(action, saved);
      notify({ tone: saved.applied.reload_error ? 'warn' : 'ok', text: source(saved.message) }, saved.applied.reload_error ? 8000 : 3000);
      reset();
    } catch (e) {
      problem = friendly(e);
      mode = 'preview';
    }
  }

  function restore(): void { if (row) { reset(); void check(row.default); } }
</script>

<AdjustmentRow title={def.label} description={def.help} titleId={ids.title} descriptionId={ids.help}>
  {#if mode === 'idle'}
    <div class="hotkey">
      <Kbd keys={current} />
      <button type="button" class="secondary" aria-describedby={ids.title} onclick={startCapture}>{t("Mudar")}</button>
      {#if !isDefault}<button type="button" class="ghost" aria-describedby={ids.title} onclick={restore}>{t("Restaurar padrão")}</button>{/if}
    </div>
  {:else if mode === 'capture'}
    <div class="hotkey">
      <div class="capture" role="status" aria-live="polite" id={ids.status}>
        {#if held.length}<Kbd keys={[...held.map((m) => MODIFIER_LABELS[m]), '…']} />{:else}{t("Pressione o novo atalho…")}{/if}
      </div>
      <button type="button" class="ghost" onclick={reset}>{t("Cancelar")}</button>
    </div>
  {:else if preview}
    <div class="hotkey">
      <Kbd keys={preview.labels} />
      <button type="button" class="primary" disabled={mode === 'saving'} onclick={save}>{mode === 'saving' ? t("Salvando…") : preview.conflicts.length ? t("Salvar mesmo assim") : t("Salvar")}</button>
      <button type="button" class="ghost" disabled={mode === 'saving'} onclick={reset}>{t("Cancelar")}</button>
    </div>
  {/if}
  {#snippet error()}
    {#if mode === 'capture'}
      <p class="muted wide">{t("Esc cancela. Atalhos já usados pelo sistema podem não chegar aqui.")}</p>
    {/if}
    {#if mode !== 'idle' && preview && preview.conflicts.length}
      <div class="conflict" role="alert">
        <strong>Conflito:</strong>
        {#each preview.conflicts as conflict, i}{#if i > 0}, {/if}“{conflict.description}”{/each}
        {t("já")} {preview.conflicts.length > 1 ? 'usam' : 'usa'} {preview.labels.join(' + ')}. {t("Se salvar, os dois disparam juntos.")}
      </div>
    {:else if mode !== 'idle' && preview?.note}
      <p class="muted wide">{t(preview.note)}</p>
    {:else if mode === 'preview' && preview}
      <p class="muted wide">{t("Sem conflito com os atalhos do Hyprland.")}</p>
    {/if}
    {#if problem}<p class="field-error" role="alert">{t(problem)}</p>{/if}
  {/snippet}
</AdjustmentRow>

<style>
  .hotkey { display:flex; align-items:center; gap:10px; flex-wrap:wrap; justify-content:flex-end; }
  .capture { min-width:190px; box-sizing:border-box; padding:7px 12px; border:1px dashed var(--accent); border-radius:9px; color:var(--text-muted); font-size:13px; text-align:center; animation:pulse 1.6s ease-in-out infinite; }
  .wide { grid-column:1 / -1; margin:0; }
  :global(.adjustment-row:has(.hotkey)) { grid-template-columns:minmax(0,1fr) auto; }
  .conflict { grid-column:1 / -1; padding:10px 12px; border:1px solid var(--warn); border-radius:10px; color:var(--text); background:color-mix(in srgb, var(--warn) 10%, transparent); font-size:13px; line-height:1.5; }
  .conflict strong { color:var(--warn); font-weight:600; }
  @keyframes pulse { 50% { border-color:var(--border); } }
  @media (prefers-reduced-motion: reduce) { .capture { animation:none; } }
</style>
