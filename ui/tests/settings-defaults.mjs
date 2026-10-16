// C-05: dica "Default: X" / "Padrão: X" nos campos das Configurações, vinda
// dos padrões que o `settings_get` devolve. Ponte Tauri inteiramente falsa:
// nunca chama a CLI, o daemon, o Ollama nem o Hyprland. Dados sintéticos.
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ?? 'playwright');
import fs from 'node:fs';
import assert from 'node:assert/strict';
const out = process.env.EVIDENCE_DIR ?? '/tmp/fw-c05-evidence'; fs.mkdirSync(out, { recursive: true });
// Mesmos valores de `fastwisper config list` sem config (Config::default()).
const defaults = {
  'ui.language': 'en', 'hub.theme': 'auto', 'audio.device': '', 'asr.languages': ['pt', 'en'], 'hotkey.max_recording_secs': 300,
  'hotkey.dictate': 'ALT+SPACE', 'hotkey.hands_free': 'ALT+SHIFT+SPACE', 'hotkey.paste_last': 'SUPER+ALT+V',
  'audio.sounds': true, 'audio.sound_volume': .4, 'overlay.enabled': true, 'overlay.position': 'bottom',
  'asr.model': '~/.local/share/fastwisper/models/ggml-large-v3-turbo-q5_0.bin', 'asr.gpu': 'auto', 'llm.enabled': true, 'llm.model': 'qwen3:4b-instruct',
  'llm.style': 'casual', 'llm.timeout_ms': 1500, 'llm.skip_below_words': 4, 'llm.format_in_terminals': true,
  'performance.vram': 'automatico', 'performance.unload_after_secs': 300, 'performance.game_llm': 'local',
  'llm.ollama_cloud': false, 'inject.clipboard_history': false, 'llm.engine': 'ollama', 'llm.endpoint': 'http://127.0.0.1:11434',
  'llm.remote_consent': false, 'llm.api_key_file': '', 'inject.paste_apps': ['*'], 'inject.copy_only_apps': [],
  'inject.harness_apps': [], 'inject.terminal_apps': ['ghostty'], 'inject.notify': true, 'dictionary.words': [], 'dictionary.replace': {}, 'snippets': [],
  'experimental.notes': false, 'inject.send_now': false, 'inject.send_now_phrases': { pt: ['enviar agora'], en: ['send now'] }
};
// Valores que diferem do padrão ganham a dica; os iguais, não.
const fixture = {
  ...defaults, 'hub.theme': 'dark', 'audio.device': 'fixture.microphone', 'audio.sound_volume': .7, 'overlay.enabled': false,
  'asr.gpu': 'cpu', 'llm.style': 'formal', 'llm.timeout_ms': 2500, 'performance.vram': 'sempre'
};
const browser = await chromium.launch({ headless: true, executablePath: process.env.BROWSER_BINARY || '/usr/bin/chromium' });
const context = await browser.newContext({ viewport: { width: 1100, height: 720 }, reducedMotion: 'reduce' });
await context.addInitScript(({ fixture, defaults }) => {
  window.isTauri = true; window.__calls = []; window.__unhandled = []; window.__values = { ...fixture };
  const vram = { mode: 'sempre', profile: 'sempre', game: false, asr: 'loaded', llm: 'loaded', vram_mb: 1800, unload_in_secs: null };
  const empty = { count: 0, p50: 0, p95: 0, max: 0 };
  let serial = 0; const callbacks = new Map(), listeners = new Map();
  window.__emit = (event, payload) => { for (const id of listeners.get(event) ?? []) callbacks.get(id)?.({ payload }); };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
  window.__TAURI_INTERNALS__ = { transformCallback: (callback) => { const id = ++serial; callbacks.set(id, callback); return id; }, invoke: async (cmd, args = {}) => {
    window.__calls.push([cmd, args]);
    const values = window.__values;
    if (cmd === 'settings_get') return { values: { ...values }, defaults, restart_keys: [] };
    if (cmd === 'settings_set') { for (const [key, value] of args.changes) values[key] = value; return { status: 'applied', restart_required: [] }; }
    if (cmd === 'hotkeys_get') return { rows: [], keys: [] };
    if (cmd === 'ui_language') return values['ui.language'] ?? 'en';
    if (cmd === 'hub_theme') return values['hub.theme'];
    if (cmd === 'theme_colors') return { background: null, foreground: null, accent: null, surface: null };
    if (cmd === 'hub_version') return '0.1.0';
    if (cmd === 'initial_destination') return null;
    if (cmd === 'daemon_view') return { available: true, state: 'idle', hands_free: false, note_session: false, asr: 'ready', vram, last: null };
    if (cmd === 'vram_total') return 8192;
    if (cmd === 'microphones') return [{ node_name: 'fixture.microphone', description: 'USB Headset', chosen: true }];
    if (cmd === 'history_stats') return { days: args.days, consecutive_weeks: 2, words: 420, ppm: 96, empty: false, total: 12, outcomes: [], deliveries: [], profiles: [], apps: [], asr_ms: empty, llm_ms: empty, e2e_ms: empty, load_ms: empty, llm_load_ms: empty };
    if (cmd === 'notes_list') return [];
    if (cmd === 'doctor_summary') return { verdict: 'Tudo certo.', ok: 8, notices: [] };
    if (cmd === 'asr_models') return [];
    if (cmd === 'asr_gpus') return ['Example Vulkan GPU'];
    if (cmd === 'ollama_models') return ['qwen3:4b-instruct'];
    if (cmd === 'ollama_cloud') return { cloud: false, disabled: true, error: null };
    if (cmd === 'omarchy_themes') return [];
    if (cmd === 'plugin:event|listen') { const group = listeners.get(args.event) ?? []; group.push(args.handler); listeners.set(args.event, group); return ++serial; }
    if (cmd === 'plugin:event|unlisten') return null;
    if (['meter_start', 'meter_stop', 'sound_test', 'mic_test'].includes(cmd)) return null;
    window.__unhandled.push(cmd); throw new Error('Unhandled mock: ' + cmd);
  } };
}, { fixture, defaults });
const page = await context.newPage(); const errors = [], network = [];
page.on('pageerror', (e) => errors.push(e.message));
page.on('console', (m) => { if (['error', 'warning'].includes(m.type())) errors.push(m.text()); });
const url = process.env.HUB_URL || 'http://127.0.0.1:5199/';
page.on('request', (r) => { if (!r.url().startsWith(new URL(url).origin)) network.push(r.url()); });
await page.goto(url);
await page.getByRole('button', { name: 'Settings', exact: true }).waitFor();
await page.evaluate(() => document.fonts.ready);

const help = (key) => page.locator(`#campo-${key.replace(/[^a-z_]/g, '-')}-help`);
const names = {
  en: { general: 'General', system: 'System', hint: 'Default', on: 'On', automatic: 'Automatic (best GPU)' },
  'pt-BR': { general: 'Geral', system: 'Sistema', hint: 'Padrão', on: 'Ligado', automatic: 'Automático (melhor GPU)' }
};
await page.getByRole('button', { name: 'Settings', exact: true }).click();
for (const language of ['en', 'pt-BR']) {
  const n = names[language];
  if (language === 'pt-BR') {
    await page.getByRole('dialog').getByRole('button', { name: 'General', exact: true }).click();
    await page.getByLabel('FastWisper language', { exact: true }).selectOption('pt-BR');
    await page.getByRole('dialog').getByRole('button', { name: n.general, exact: true }).waitFor();
  }
  await page.getByRole('dialog').getByRole('button', { name: n.general, exact: true }).click();
  // Rótulo da opção padrão, lido do próprio <select> (catálogo traduzido).
  const optionLabel = (key, value) => page.locator(`#campo-${key.replace(/[^a-z_]/g, '-')} option[value="${value}"]`).textContent();
  await help('audio.sound_volume').filter({ hasText: `${n.hint}: 40%.` }).waitFor();
  assert.ok((await help('overlay.enabled').textContent()).trim().endsWith(`${n.hint}: ${n.on}.`), language);
  // Iguais ao padrão: sem dica.
  for (const key of ['audio.sounds', 'hotkey.max_recording_secs']) {
    assert.ok(!(await help(key).textContent()).includes(`${n.hint}:`), `${language}: ${key}`);
  }
  // O idioma do FastWisper difere do padrão só em pt-BR.
  assert.equal((await help('ui.language').textContent()).includes(`${n.hint}: English.`), language === 'pt-BR');
  if (language === 'en') {
    await page.waitForTimeout(150); await page.screenshot({ path: `${out}/en-general.png` });
    await help('audio.sound_volume').scrollIntoViewIfNeeded(); await page.waitForTimeout(150);
    await page.screenshot({ path: `${out}/en-general-sounds.png` });
  }
  await page.getByRole('dialog').getByRole('button', { name: n.system, exact: true }).click();
  await help('llm.timeout_ms').filter({ hasText: `${n.hint}: 1500 ms.` }).waitFor();
  assert.ok((await help('asr.gpu').textContent()).trim().endsWith(`${n.hint}: ${n.automatic}.`), language);
  assert.ok((await help('llm.style').textContent()).trim().endsWith(`${n.hint}: ${(await optionLabel('llm.style', 'casual')).trim()}.`), language);
  assert.ok((await help('performance.vram').textContent()).trim().endsWith(`${n.hint}: ${(await optionLabel('performance.vram', 'automatico')).trim()}.`), language);
  for (const key of ['llm.enabled', 'llm.skip_below_words', 'performance.unload_after_secs']) {
    assert.ok(!(await help(key).textContent()).includes(`${n.hint}:`), `${language}: ${key}`);
  }
  if (language === 'en') { await page.waitForTimeout(150); await page.screenshot({ path: `${out}/en-system.png` }); }
}
// Voltar ao padrão tira a dica.
await page.locator('#campo-llm-timeout_ms').fill('1500');
await page.locator('#campo-llm-timeout_ms').press('Enter');
await page.waitForFunction(() => !document.querySelector('#campo-llm-timeout_ms-help')?.textContent.includes('Padrão:'));
assert.deepEqual(errors, []);
assert.deepEqual(network, []);
assert.deepEqual(await page.evaluate(() => window.__unhandled), []);
await browser.close();
console.log(`settings-defaults: PASS (dica de padrão em en e pt-BR; capturas em ${out})`);
