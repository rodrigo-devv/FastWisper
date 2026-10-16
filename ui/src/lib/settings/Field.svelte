<script lang="ts">
  import { t, source, locale } from "$lib/i18n/index.svelte";
  import type { Snippet } from 'svelte';
  import AdjustmentRow from './AdjustmentRow.svelte';
  import ThemeCards from './ThemeCards.svelte';
  import type { Choice } from './fields';
  import { fieldDef } from './fields';
  import { settings, saveKey } from './store.svelte';
  import ListEditor from './ListEditor.svelte';
  import MapEditor from './MapEditor.svelte';
  import PhrasesEditor from './PhrasesEditor.svelte';
  import SnippetsEditor from './SnippetsEditor.svelte';

  let { key, disabled = false, wip = false, children: extra, options, help }: { key: string; disabled?: boolean; wip?: boolean; children?: Snippet; options?: Choice[]; help?: string } = $props();
  const def = $derived(fieldDef(key));
  const value = $derived(key === "ui.language" ? locale() : settings.values[key]);
  const id = $derived(`campo-${key.replace(/[^a-z_]/g, '-')}`);
  const restart = $derived(settings.restartKeys.includes(key));
  const busy = $derived(disabled || !settings.loaded || settings.saving[key]);
  const choices = $derived(options ?? def.options);
  let draft = $state('');
  let localError = $state('');

  $effect(() => { draft = value === undefined ? '' : String(value); });

  function commitNumber(): void {
    const n = Number(draft);
    if (draft.trim() === '' || !Number.isFinite(n) || !Number.isInteger(n)) { localError = source("Digite um número inteiro."); return; }
    if ((def.min !== undefined && n < def.min) || (def.max !== undefined && n > def.max)) {
      localError = source("Use de {0} a {1}.", def.min, def.max);
      return;
    }
    localError = '';
    if (n !== value) void saveKey(key, n);
  }

  function commitText(): void {
    if (draft !== value) void saveKey(key, draft.trim());
  }

  async function toggleMulti(option: string, on: boolean): Promise<boolean> {
    const current = Array.isArray(value) ? (value as string[]) : [];
    const next = on ? [...current, option] : current.filter((v) => v !== option);
    if (next.length === 0) { localError = source("Escolha ao menos um."); return false; }
    localError = '';
    return saveKey(key, next);
  }

  const labelText = (v: unknown) => choices?.find((o) => o.value === v)?.label ?? String(v);
  const fmt = (v: unknown) => (def.kind === 'toggle' ? t(v === true ? "Ligado" : "Desligado") : def.percent ? `${Math.round(Number(v) * 100)}%` : def.unit ? `${v} ${def.unit}` : choices ? labelText(v) : String(v));
  // "Padrão: X" ao fim da ajuda quando o valor difere do padrão real
  // (`settings_get` → `Config::default()`), só em controles de valor único.
  const fallback = $derived(settings.defaults[key]);
  const hinted = $derived(['toggle', 'slider', 'select', 'number', 'text'].includes(def.kind) || (def.kind === 'custom' && options !== undefined));
  const defaultHint = $derived(settings.loaded && hinted && ['string', 'number', 'boolean'].includes(typeof fallback) && fallback !== ''
    && JSON.stringify(value) !== JSON.stringify(fallback) ? t("Padrão: {0}.", fmt(fallback)) : '');
  const description = $derived([help ?? def.help, defaultHint].filter(Boolean).join(' '));
</script>

<AdjustmentRow {wip} title={def.label} {description} titleId={`${id}-label`} descriptionId={`${id}-help`} controlId={(['toggle', 'slider', 'select', 'number', 'text'].includes(def.kind) || options) && key !== 'hub.theme' ? id : undefined} fullWidth={key === 'hub.theme' || ['list', 'map', 'phrases', 'snippets'].includes(def.kind)}>
  {#snippet metadata()}
    {#if restart}<span class="badge" title={t("Só vale depois de reiniciar o FastWisper")}>{t("exige reinício")}</span>{/if}
    {#if settings.pendingRestart.includes(key)}<span class="badge warn">{t("reinício pendente")}</span>{/if}
    {#if settings.saving[key]}<span class="saving" aria-live="polite">{t("salvando…")}</span>{/if}
  {/snippet}
  {#snippet children()}
    {#if key === 'hub.theme'}<ThemeCards />
    {:else if def.kind === 'toggle'}
      <button id={id} type="button" role="switch" class="switch" aria-checked={value === true} aria-labelledby="{id}-label" aria-describedby="{id}-help" disabled={busy} onclick={() => saveKey(key, value !== true)}><span></span></button>
    {:else if def.kind === 'slider'}
      <div class="slider">
        <input id={id} type="range" min={def.min} max={def.max} step={def.step} value={Number(value ?? 0)} aria-describedby="{id}-help" disabled={busy}
          oninput={(e) => (draft = e.currentTarget.value)} onchange={(e) => saveKey(key, Number(e.currentTarget.value))} />
        <output for={id}>{fmt(draft === '' ? value : Number(draft))}</output>
      </div>
    {:else if def.kind === 'select' || options}
      <select id={id} aria-describedby="{id}-help" disabled={busy || (choices?.length ?? 0) < 2} value={value} onchange={async (e) => { const input = e.currentTarget; if (!await saveKey(key, input.value)) input.value = String(key === "ui.language" ? locale() : settings.values[key] ?? ''); }}>
        {#each choices ?? [] as option}<option value={option.value}>{option.label}</option>{/each}
      </select>
    {:else if def.kind === 'number'}
      <div class="number">
        <input id={id} type="number" inputmode="numeric" min={def.min} max={def.max} step={def.step ?? 1} value={draft} oninput={(e) => draft = e.currentTarget.value} aria-describedby="{id}-help" aria-invalid={localError !== ''} disabled={busy}
          onchange={commitNumber} onkeydown={(e) => e.key === 'Enter' && commitNumber()} />
        {#if def.unit}<span class="unit">{def.unit}</span>{/if}
      </div>
    {:else if def.kind === 'text'}
      <input id={id} type="text" list={def.suggestions ? `${id}-sugestoes` : undefined} bind:value={draft} aria-describedby="{id}-help" disabled={busy} spellcheck="false" autocomplete="off"
        onchange={commitText} onkeydown={(e) => e.key === 'Enter' && commitText()} />
      {#if def.suggestions}<datalist id="{id}-sugestoes">{#each def.suggestions as s}<option value={s}></option>{/each}</datalist>{/if}
    {:else if def.kind === 'multi'}
      <div class="chips" role="group" aria-labelledby="{id}-label" aria-describedby="{id}-help">
        {#each choices ?? [] as option}
          <label class="chip-check">
            <input type="checkbox" checked={Array.isArray(value) && (value as string[]).includes(option.value)} disabled={busy} onchange={async (e) => { const input = e.currentTarget; if (!await toggleMulti(option.value, input.checked)) input.checked = Array.isArray(value) && (value as string[]).includes(option.value); }} />
            {option.label}
          </label>
        {/each}
      </div>
    {:else if def.kind === 'list'}
      <ListEditor {key} items={Array.isArray(value) ? (value as string[]) : []} placeholder={def.placeholder} labelledby="{id}-label" disabled={busy} />
    {:else if def.kind === 'map'}
      <MapEditor {key} entries={(value ?? {}) as Record<string, string>} placeholder={def.placeholder} labelledby="{id}-label" disabled={busy} />
    {:else if def.kind === 'phrases'}
      <PhrasesEditor {key} phrases={(value ?? {}) as Record<string, string[]>} placeholder={def.placeholder} labelledby="{id}-label" disabled={busy} />
    {:else if def.kind === 'snippets'}
      <SnippetsEditor {key} items={Array.isArray(value) ? (value as { trigger: string; expansion: string }[]) : []} labelledby="{id}-label" disabled={busy} />
    {/if}
    {@render extra?.()}
  {/snippet}
  {#snippet error()}
    {#if localError || settings.errors[key]}<p class="field-error" role="alert">{t(localError || settings.errors[key])}</p>{/if}
  {/snippet}
</AdjustmentRow>
