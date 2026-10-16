import { readFileSync } from 'node:fs';
import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';
// Versão do pacote no lugar do carimbo de hora padrão do SvelteKit: com ele,
// cada build mudava o binário do hub e o install.sh criava um degrau de
// rollback a cada reinstalação, mesmo sem mudança no código.
const { version } = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8'));
export default {
  preprocess: vitePreprocess(),
  kit: { adapter: adapter({ fallback: 'index.html' }), version: { name: version } }
};
