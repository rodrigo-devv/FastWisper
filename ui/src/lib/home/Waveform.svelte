<script lang="ts">
  import { followMeter } from '$lib/meter';
  import { onMount } from 'svelte';
  let levels = $state<number[]>(Array(24).fill(0));
  const envelope = (index: number) => { const x = (index + .5) / 24 - .5; return .35 + .65 * Math.cos(x * Math.PI); };
  onMount(() => {
    let closed = false, stop: (() => void) | undefined;
    void followMeter(frame => {
      const v = Math.max(0, Math.min(1, (frame.level - 0.2) / 0.55));
      const next = levels.slice();
      for (let i = 0; i < 11; i++) next[i] = next[i + 1];
      for (let i = 23; i > 12; i--) next[i] = next[i - 1];
      next[11] = v; next[12] = v; levels = next;
    }).then(unlisten => { if (closed) unlisten(); else stop = unlisten; });
    return () => { closed = true; stop?.(); };
  });
</script>
<div class="capsule" aria-hidden="true"><div class="wave">{#each levels as level, i}<span style:height={`${Math.max(3, Math.round(24 * Math.max(.22 * envelope(i), envelope(i) * level)))}px`}></span>{/each}</div></div>
<style>
  .capsule { display:grid; place-items:center; width:180px; height:40px; box-sizing:border-box; border-radius:20px; border:1px solid var(--border); background:var(--bg); }
  .wave { display:flex; align-items:center; gap:3px; height:24px; }
  span { width:3px; border-radius:1.5px; background:var(--text); transition:height 70ms linear; }
  @media(prefers-reduced-motion:reduce) { span { transition:none; } }
</style>
