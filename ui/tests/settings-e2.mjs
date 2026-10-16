// QA dos atalhos editáveis (E2) com ponte Tauri inteiramente falsa: nunca
// chama a CLI, o daemon, o hyprctl nem o Hyprland. Gera as capturas de aceite.
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ?? 'playwright');
import fs from 'node:fs';
import assert from 'node:assert/strict';
const out = process.env.EVIDENCE_DIR ?? '/tmp/fw-e2-evidence'; fs.mkdirSync(out, { recursive: true });
const fixture = {
  "ui.language": "pt-BR",
  'hub.theme': 'dark', 'audio.device': 'fixture.microphone', 'asr.languages': ['pt', 'en'], 'hotkey.max_recording_secs': 300,
  'hotkey.dictate': 'ALT+SPACE', 'hotkey.hands_free': 'ALT+SHIFT+SPACE', 'hotkey.paste_last': 'SUPER+ALT+V',
  'audio.sounds': true, 'audio.sound_volume': .4, 'overlay.enabled': true, 'overlay.position': 'bottom',
  'asr.model': '/fixture/ggml-small.bin', 'asr.gpu': 'auto', 'llm.enabled': true, 'llm.model': 'qwen3:4b-instruct',
  'llm.style': 'casual', 'llm.timeout_ms': 2000, 'llm.skip_below_words': 3, 'llm.format_in_terminals': true,
  'performance.vram': 'automatico', 'performance.unload_after_secs': 300, 'performance.game_llm': 'local',
  'llm.ollama_cloud': false, 'inject.clipboard_history': false, 'llm.engine': 'ollama', 'llm.endpoint': 'http://127.0.0.1:11434',
  'llm.remote_consent': false, 'llm.api_key_file': '', 'inject.paste_apps': ['*'], 'inject.copy_only_apps': [],
  'inject.harness_apps': [], 'inject.terminal_apps': ['ghostty'], 'inject.notify': true, 'dictionary.words': [], 'dictionary.replace': {}, 'snippets': [],
  'experimental.notes': true
};
// Recorte da tabela do fw_core (nome, keycode, KeyboardEvent.code, rótulo).
const keys = [
  ['D', 40, 'KeyD', 'D'], ['V', 55, 'KeyV', 'V'], ['F9', 75, 'F9', 'F9'], ['SPACE', 65, 'Space', 'Espaço'], ['RETURN', 36, 'Enter', 'Enter']
].map(([name, keycode, code, label]) => ({ name, keycode, code, label }));
const browser = await chromium.launch({ headless: true, executablePath: process.env.BROWSER_BINARY || '/usr/bin/chromium' });
const context = await browser.newContext({ viewport: { width: 1100, height: 720 }, reducedMotion: 'reduce' });
await context.addInitScript(({ fixture, keys }) => {
  window.isTauri = true; window.__calls = []; window.__values = { ...fixture };
  const vram = { mode: 'automatico', profile: 'sob_demanda', game: false, asr: 'loaded', llm: 'unloaded', vram_mb: 1800, unload_in_secs: null };
  const MODS = { SUPER: 'Super', CTRL: 'Ctrl', ALT: 'Alt', SHIFT: 'Shift' };
  const ORDER = ['SUPER', 'CTRL', 'ALT', 'SHIFT'];
  const labels = (combo) => combo.split('+').map((p) => MODS[p] ?? keys.find((k) => k.name === p)?.label ?? p);
  const canonical = (combo) => { const parts = combo.toUpperCase().split('+'); const key = parts.pop(); return [...ORDER.filter((m) => parts.includes(m)), key].join('+'); };
  const ACTIONS = { dictate: 'Atalho para ditar', hands_free: 'Mãos livres', paste_last: 'Colar de novo' };
  const DEFAULTS = { dictate: 'ALT+SPACE', hands_free: 'ALT+SHIFT+SPACE', paste_last: 'SUPER+ALT+V' };
  const FOREIGN = { 'SUPER+SPACE': [{ description: 'Omarchy menu', combo: 'SUPER+SPACE' }] };
  const rows = () => Object.keys(ACTIONS).map((action) => ({ action, key: `hotkey.${action}`, combo: window.__values[`hotkey.${action}`], labels: labels(window.__values[`hotkey.${action}`]), default: DEFAULTS[action], default_labels: labels(DEFAULTS[action]) }));
  function preview(action, combo) {
    const c = canonical(combo);
    const parts = c.split('+');
    if (!parts.slice(0, -1).some((m) => m !== 'SHIFT')) throw 'use ao menos um modificador além de Shift (Super, Ctrl ou Alt): sem ele, o Hyprland engole a tecla durante a digitação';
    for (const other of Object.keys(ACTIONS)) if (other !== action && window.__values[`hotkey.${other}`] === c) throw `${labels(c).join(' + ')} já é o atalho de ${ACTIONS[other]}; escolha outro`;
    return { action, combo: c, labels: labels(c), conflicts: FOREIGN[c] ?? [], checked: true };
  }
  let serial = 0; const callbacks = new Map(), listeners = new Map();
  window.__emit = (event, payload) => { for (const id of listeners.get(event) ?? []) callbacks.get(id)?.({ payload }); };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
  window.__TAURI_INTERNALS__ = { transformCallback: (callback) => { const id = ++serial; callbacks.set(id, callback); return id; }, invoke: async (cmd, args = {}) => {
    window.__calls.push([cmd, args]);
    const values = window.__values;
    if (cmd === 'settings_get') return { values: { ...values }, defaults: fixture, restart_keys: [] };
    if (cmd === 'settings_set') { for (const [key, value] of args.changes) values[key] = value; return { status: 'applied', restart_required: [] }; }
    if (cmd === 'hotkeys_get') return { rows: rows(), keys };
    if (cmd === 'hotkey_preview') return preview(args.action, args.combo);
    if (cmd === 'hotkey_save') {
      const p = preview(args.action, args.combo);
      if (p.conflicts.length && !args.confirm) throw 'confirme para gravar mesmo assim';
      values[`hotkey.${args.action}`] = p.combo;
      return { combo: p.combo, labels: p.labels, applied: { status: 'reloaded' }, message: 'Atalhos aplicados: o Hyprland recarregou.' };
    }
    if (cmd === 'ui_language') return 'pt-BR';
    if (cmd === 'hub_theme') return values['hub.theme'];
    if (cmd === 'theme_colors') return { background: null, foreground: null, accent: null, surface: null };
    if (cmd === 'hub_version') return '0.1.0';
    if (cmd === 'initial_destination') return null;
    if (cmd === 'daemon_view') return { available: true, state: window.__recording ? 'recording' : 'idle', hands_free: false, note_session: false, asr: 'ready', vram, last: null };
    if (cmd === 'vram_total') return 16384;
    if (cmd === 'microphones') return [{ node_name: 'fixture.microphone', description: 'Microfone padrão', chosen: true }];
    if (cmd === 'history_stats') return { days: args.days, consecutive_weeks: 1, words: 123, ppm: 82, empty: false, total: 4, outcomes: [], deliveries: [], profiles: [], apps: [], asr_ms: { count: 0, p50: 0, p95: 0, max: 0 }, llm_ms: { count: 0, p50: 0, p95: 0, max: 0 }, e2e_ms: { count: 0, p50: 0, p95: 0, max: 0 }, load_ms: { count: 0, p50: 0, p95: 0, max: 0 }, llm_load_ms: { count: 0, p50: 0, p95: 0, max: 0 } };
    if (cmd === 'notes_list') return [];
    if (cmd === 'plugin:event|listen') { const group = listeners.get(args.event) ?? []; group.push(args.handler); listeners.set(args.event, group); return ++serial; }
    if (cmd === 'plugin:event|unlisten') return null;
    if (['meter_start', 'meter_stop', 'sound_test', 'mic_test'].includes(cmd)) return null;
    throw new Error('Unhandled mock: ' + cmd);
  } };
}, { fixture, keys });
const page = await context.newPage(); const errors = [], network = [];
page.on('pageerror', (e) => errors.push(e.message));
page.on('console', (m) => { if (['error', 'warning'].includes(m.type())) errors.push(m.text()); });
const url = process.env.HUB_URL || 'http://127.0.0.1:5197/';
page.on('request', (r) => { if (!r.url().startsWith(new URL(url).origin)) network.push(r.url()); });
await page.goto(url);
await page.getByRole('button', { name: 'Configurações', exact: true }).waitFor();
await page.evaluate(() => document.fonts.ready);
// Cartão de estado e composer mostram o atalho padrão.
assert.match(await page.locator('.status .hold').innerText(), /Segure\s*Alt\s*\+\s*Espaço/);
await page.locator('.composer .hint').waitFor();
assert.match(await page.locator('.composer .hint').innerText(), /segure\s*Alt\s*\+\s*Espaço/);

const dialog = page.getByRole('dialog');
const rowOf = (title) => page.locator('.adjustment-row').filter({ has: page.getByText(title, { exact: true }) });
const shot = async (name) => { await page.waitForTimeout(150); await page.screenshot({ path: `${out}/${name}.png` }); };
async function openGeral(theme) {
  await page.getByRole('button', { name: 'Configurações', exact: true }).click();
  await page.getByRole('dialog', { name: 'Geral', exact: true }).waitFor();
  await page.getByRole('radio', { name: theme === 'dark' ? 'Escuro' : 'Claro', exact: true }).check();
  await page.waitForFunction((t) => document.documentElement.dataset.theme === t, theme);
  await rowOf('Atalho para ditar').scrollIntoViewIfNeeded();
  await page.locator('.content.settings-modal').evaluate((el) => { const row = el.querySelector('.adjustment-row:has(.hotkey)'); el.scrollTop = row.offsetTop - 120; });
}

for (const theme of ['dark', 'light']) {
  await openGeral(theme);
  const dictate = rowOf('Atalho para ditar');
  assert.equal(await dictate.getByRole('button', { name: 'Restaurar padrão' }).count(), 0, 'padrão sem botão de restaurar');
  await shot(`${theme}-ditado`);
  // Captura: Esc cancela sem fechar o modal.
  await dictate.getByRole('button', { name: 'Mudar', exact: true }).click();
  await dictate.getByText('Pressione o novo atalho…').waitFor();
  await shot(`${theme}-captura`);
  await page.keyboard.press('Escape');
  await dictate.getByRole('button', { name: 'Mudar', exact: true }).waitFor();
  assert.equal(await page.getByRole('dialog', { name: 'Geral', exact: true }).count(), 1, 'Esc da captura não fecha o modal');
  // Só Shift: recusado na hora; duplicado: recusado com o nome da outra ação.
  await dictate.getByRole('button', { name: 'Mudar', exact: true }).click();
  await page.keyboard.press('Shift+KeyD');
  await dictate.getByText(/modificador além de Shift/).waitFor();
  await page.keyboard.press('Alt+Shift+Space');
  await dictate.getByText(/já é o atalho de Mãos livres/).waitFor();
  // Conflito com bind de outro programa: prévia avisa e pede confirmação.
  await page.keyboard.press('Meta+Space');
  await dictate.getByRole('alert').filter({ hasText: 'Omarchy menu' }).waitFor();
  await dictate.getByRole('button', { name: 'Salvar mesmo assim', exact: true }).waitFor();
  await shot(`${theme}-conflito`);
  await dictate.getByRole('button', { name: 'Cancelar', exact: true }).click();
  // Sem conflito: salva e o hub inteiro passa a mostrar o novo atalho.
  await dictate.getByRole('button', { name: 'Mudar', exact: true }).click();
  await page.keyboard.press('Control+Alt+KeyD');
  await dictate.getByText('Sem conflito com os atalhos do Hyprland.').waitFor();
  await shot(`${theme}-previa`);
  await dictate.getByRole('button', { name: 'Salvar', exact: true }).click();
  await dictate.getByRole('button', { name: 'Restaurar padrão', exact: true }).waitFor();
  const save = await page.evaluate(() => window.__calls.filter((c) => c[0] === 'hotkey_save').at(-1));
  assert.deepEqual(save, ['hotkey_save', { action: 'dictate', combo: 'CTRL+ALT+D', confirm: false }]);
  assert.match(await dictate.locator('.keys').innerText(), /Ctrl\s*\+\s*Alt\s*\+\s*D/);
  await shot(`${theme}-salvo`);
  await page.keyboard.press('Escape');
  await page.getByRole('dialog').waitFor({ state: 'detached' });
  assert.match(await page.locator('.status .hold').innerText(), /Segure\s*Ctrl\s*\+\s*Alt\s*\+\s*D/);
  assert.match(await page.locator('.composer .hint').innerText(), /Ctrl\s*\+\s*Alt\s*\+\s*D/);
  await shot(`${theme}-cartao`);
  // Restaurar padrão passa pela mesma prévia.
  await openGeral(theme);
  await rowOf('Atalho para ditar').getByRole('button', { name: 'Restaurar padrão', exact: true }).click();
  await rowOf('Atalho para ditar').getByRole('button', { name: 'Salvar', exact: true }).click();
  await rowOf('Atalho para ditar').getByRole('button', { name: 'Mudar', exact: true }).waitFor();
  assert.equal(await page.evaluate(() => window.__values['hotkey.dictate']), 'ALT+SPACE');
  await page.keyboard.press('Escape');
  await page.getByRole('dialog').waitFor({ state: 'detached' });
}
// Gravando: a dica usa o atalho configurado.
await page.evaluate(() => { window.__values['hotkey.dictate'] = 'SUPER+F9'; });
await page.evaluate(() => window.dispatchEvent(new Event('focus')));
await page.evaluate(() => window.__emit('daemon-status', { available: true, state: 'recording', hands_free: false, note_session: false, asr: 'ready', vram: null, last: null }));
await page.locator('.status').getByText('Solte Super+F9 para transcrever. Esc cancela.').waitFor({ timeout: 5000 });
assert.equal(await page.locator('vite-error-overlay').count(), 0);
assert.deepEqual(errors, []);
assert.deepEqual(network, []);
fs.writeFileSync(`${out}/checks.json`, JSON.stringify({ url, viewport: [1100, 720], browser: 'Playwright/Chromium headless', bridge: 'Tauri falsa; nenhum comando real executado', checks: ['linhas de Ditado com atalho atual', 'captura (Esc cancela sem fechar o modal)', 'só Shift recusado', 'duplicado entre os três recusado', 'conflito com bind de outro programa avisado na prévia', 'salvar sem conflito envia confirm=false', 'cartão de estado e composer mostram o atalho salvo', 'restaurar padrão', 'dica de gravação com o atalho configurado'], errors, network }, null, 2) + '\n');
await browser.close();
console.log('settings-e2: PASS');
