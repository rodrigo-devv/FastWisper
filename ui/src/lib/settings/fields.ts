import { t } from "$lib/i18n/index.svelte";
import catalog from './fields.json';

// Cada campo é uma chave de `fastwisper config`; o teste do Tauri confere.
export type Kind = 'toggle' | 'slider' | 'select' | 'number' | 'text' | 'multi' | 'list' | 'map' | 'phrases' | 'snippets' | 'custom' | 'action';
export interface Choice { value: string; label: string; help?: string }
export interface FieldDef {
  key: string;
  page: string;
  kind: Kind;
  label: string;
  help: string;
  options?: Choice[];
  /** Catálogo dinâmico das paletas locais, documentado também para a CLI. */
  dynamic_options?: string;
  value_pattern?: string;
  min?: number;
  max?: number;
  step?: number;
  unit?: string;
  percent?: boolean;
  placeholder?: string;
  suggestions?: string[];
}

export const fields = catalog as FieldDef[];

export function fieldDef(key: string): FieldDef {
  const def = fields.find((f) => f.key === key);
  if (!def) throw new Error(t("campo fora do catálogo: {0}", key));
  return {
    ...def, label: t(def.label), help: t(def.help),
    placeholder: def.placeholder ? t(def.placeholder) : undefined,
    unit: def.unit ? t(def.unit) : undefined,
    options: def.options?.map(option => ({ ...option, label: t(option.label), help: option.help ? t(option.help) : undefined }))
  };
}
