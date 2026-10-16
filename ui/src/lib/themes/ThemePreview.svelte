<script lang="ts">
  import { deriveTokens, fixedColors, type ThemeColors } from '$lib/backend';
  let { colors, mode = 'dark' }: { colors?: ThemeColors | null; mode?: 'auto' | 'dark' | 'light' } = $props();
  const palettes = $derived(colors ? [colors] : mode === 'auto' ? [fixedColors.dark, fixedColors.light] : [fixedColors[mode]]);
  const styleFor = (palette: ThemeColors) => Object.entries(deriveTokens(palette)).map(([key, value]) => `${key}:${value}`).join(';');
</script>
<div class="preview" aria-hidden="true">
  {#each palettes as palette}
    <div class="mini" style={styleFor(palette)}>
      <div class="menu"><span class="brand"></span><i></i><i></i><i></i><span class="state"></span></div>
      <div class="body"><span class="title"></span><i></i><div class="lines"><i></i><i></i><i></i></div><div class="composer"><span></span><b></b></div></div>
    </div>
  {/each}
</div>
<style>
  .preview { display:flex; aspect-ratio:2 / 1; overflow:hidden; border-radius:9px; }
  .mini { flex:1; min-width:0; display:flex; background:var(--bg); }
  .menu { width:28%; background:var(--surface); border-right:1px solid var(--border); padding:12px 8px; display:flex; flex-direction:column; gap:6px; }
  .brand { height:6px; width:78%; background:var(--accent); border-radius:2px; margin-bottom:6px; }
  i { display:block; background:var(--text-muted); height:3px; border-radius:2px; opacity:.65; }
  .menu i:nth-child(3) { width:70%; }
  .state { height:14px; margin-top:auto; background:var(--bg); border:1px solid var(--border); border-radius:3px; }
  .body { flex:1; min-width:0; padding:14px 12px 10px; display:flex; flex-direction:column; gap:6px; }
  .title { width:65%; height:6px; background:var(--text); border-radius:2px; }
  .body > i { width:40%; }
  .lines { display:flex; flex-direction:column; gap:6px; margin-top:5px; }
  .lines i { background:var(--border); height:2px; }
  .composer { display:flex; align-items:center; justify-content:space-between; gap:5px; padding:7px; height:24px; box-sizing:border-box; margin-top:auto; border:1px solid var(--border); background:var(--surface); border-radius:5px; }
  .composer span { width:66%; height:3px; background:var(--text-muted); border-radius:2px; }
  b { width:8px; height:8px; border-radius:50%; background:var(--accent); }
</style>
