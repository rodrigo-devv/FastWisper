<script lang="ts">
  import { t, locale } from "$lib/i18n/index.svelte";
  import { Copy, Trash2 } from '@lucide/svelte';
  // Texto somente do banco opt-in de notas, nunca do histórico de ditado.
  let { time, preview, words, language, compact = false, oncopy, ondelete }: {
    time: string; preview: string; words: number; language: string; compact?: boolean;
    oncopy?: () => void; ondelete?: () => void;
  } = $props();
</script>

<li class:compact>
  <span class="time">{time}</span>
  <div class="content"><p class="preview">{preview}</p><p class="meta">{words.toLocaleString(locale())} {t("palavras ·")} {language}</p></div>
  <div class="actions">
    <button type="button" aria-label={t("Copiar nota")} title={t("Copiar nota")} onclick={oncopy} disabled={!oncopy}><Copy size={15} aria-hidden="true" /></button>
    <button type="button" aria-label={t("Apagar nota")} title={t("Apagar nota")} onclick={ondelete} disabled={!ondelete}><Trash2 size={15} aria-hidden="true" /></button>
  </div>
</li>

<style>
  li { display:flex; gap:20px; padding:14px 0; border-bottom:1px solid var(--border); align-items:flex-start; }
  .time { width:72px; flex:none; font-size:12.5px; color:var(--text-muted); padding-top:2px; }
  .content { flex:1; min-width:0; }
  p { margin:0; }
  .preview { font-size:14px; line-height:1.5; overflow-wrap:anywhere; }
  .meta { margin-top:4px; color:var(--text-muted); font-size:12px; }
  .actions { display:flex; gap:8px; }
  button { border:0; background:transparent; color:var(--text-muted); padding:4px; border-radius:5px; cursor:pointer; }
  button:hover:not(:disabled) { color:var(--text); background:var(--hover); }
  button:disabled { opacity:.4; cursor:default; }
  .compact { padding:10px 0; }
  .compact .preview { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  @media (max-width:520px) { li { gap:10px; } .time { width:48px; } }
</style>
