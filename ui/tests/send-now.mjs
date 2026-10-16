// E1 · "Enviar agora" no hub. Ponte inteiramente falsa: não chama CLI/daemon,
// não grava a config do dono e nenhuma tecla vai a janela nenhuma.
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ?? 'playwright');
import fs from 'node:fs';
import assert from 'node:assert/strict';
const out = process.env.EVIDENCE_DIR ?? '/tmp/fw-e1-evidence';
fs.mkdirSync(out, { recursive: true });
const fixture = {
  "ui.language": "pt-BR",
  'hub.theme': 'dark', 'llm.enabled': true, 'llm.engine': 'ollama', 'llm.ollama_cloud': false,
  'llm.format_in_terminals': true, 'inject.terminal_apps': ['ghostty', 'kitty', 'Alacritty', 'foot'],
  'inject.paste_apps': ['*'], 'inject.copy_only_apps': [], 'inject.notify': true,
  'inject.send_now': false, 'inject.send_now_phrases': { pt: ['enviar agora'], en: ['send now'] }
};
const browser = await chromium.launch({ headless: true, executablePath: process.env.BROWSER_BINARY || '/usr/bin/chromium' });
const context = await browser.newContext({ viewport: { width: 1100, height: 720 }, reducedMotion: 'reduce' });
await context.addInitScript(({ fixture }) => {
  const clone = value => JSON.parse(JSON.stringify(value)); // como a ponte do Tauri
  window.isTauri = true; window.__calls = []; window.__values = clone(fixture);
  let serial = 0; const callbacks = new Map(), listeners = new Map();
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
  window.__TAURI_INTERNALS__ = {
    transformCallback: callback => { const id = ++serial; callbacks.set(id, callback); return id; },
    invoke: async (command, args = {}) => {
      window.__calls.push([command, clone(args)]);
      if (command === 'settings_get') return { values: clone(window.__values), defaults: fixture, restart_keys: [] };
      if (command === 'settings_set') {
        for (const [key, value] of args.changes) window.__values[key] = clone(value);
        return { status: 'applied', restart_required: [] };
      }
      if (command === 'ui_language') return 'pt-BR';
      if (command === 'hub_theme') return window.__values['hub.theme'];
      if (command === 'theme_colors') return { background: null, foreground: null, accent: null, surface: null };
      if (command === 'vram_total') return 8192;
      if (command === 'hub_version') return '0.1.0';
      if (command === 'initial_destination') return { kind: 'settings', tab: 'vibe-coding' };
      if (command === 'daemon_view') return { available: true, state: 'idle', hands_free: false, asr: 'ready', vram: null, last: null };
      if (command === 'notes_list') return [];
      if (command === 'history_stats') return { consecutive_weeks: 0, words: 0, ppm: 0 };
      if (command === 'plugin:event|listen') { const group = listeners.get(args.event) ?? []; group.push(args.handler); listeners.set(args.event, group); return ++serial; }
      if (command === 'plugin:event|unlisten') return null;
      throw new Error('Comando fora do mock: ' + command);
    }
  };
}, { fixture });
const page = await context.newPage(); const errors = [];
page.on('pageerror', error => errors.push(error.message));
page.on('console', message => { if (['error', 'warning'].includes(message.type())) errors.push(message.text()); });
page.on('request', request => { if (!request.url().startsWith(process.env.HUB_URL || 'http://127.0.0.1:5194/')) errors.push('requisição externa: ' + request.url()); });
const url = process.env.HUB_URL || 'http://127.0.0.1:5194/';
await page.goto(url);
await page.getByRole('dialog', { name: 'Vibe coding', exact: true }).waitFor();
await page.evaluate(() => document.fonts.ready);
const writes = () => page.evaluate(() => window.__calls.filter(([command]) => command === 'settings_set'));

// A linha informativa "Enter nunca é enviado" virou o switch, desligado por padrão.
assert.equal(await page.getByText('Enter nunca é enviado', { exact: true }).count(), 0);
const sendNow = page.getByRole('switch', { name: 'Enviar com “enviar agora”', exact: true });
assert.equal(await sendNow.getAttribute('aria-checked'), 'false');
const help = await page.locator('#campo-inject-send_now-help').textContent();
for (const limit of ['terminal', 'Enter', 'foco', 'texto comum']) assert.ok(help.includes(limit), help);
const phrasesRow = page.locator('.adjustment-row').filter({ hasText: 'Frases do “enviar agora”' });
assert.ok((await phrasesRow.textContent()).includes('“enviar agora”, “send now”') || (await phrasesRow.textContent()).includes('“send now”, “enviar agora”'));

// Capturas da tela principal, Escuro e Claro, com o switch desligado e ligado.
async function setTheme(theme) {
  await page.getByRole('button', { name: 'Geral', exact: true }).click();
  await page.getByRole('radio', { name: { dark: 'Escuro', light: 'Claro' }[theme], exact: true }).check();
  await page.waitForFunction(theme => document.documentElement.dataset.theme === theme, theme);
  await page.waitForTimeout(2600); // expira o toast do mock
  await page.getByRole('button', { name: 'Vibe coding', exact: true }).click();
  await page.waitForTimeout(100);
}
await setTheme('dark');
await page.screenshot({ path: `${out}/escuro-vibe-coding.png` });

await sendNow.click();
assert.deepEqual((await writes()).at(-1), ['settings_set', { changes: [['inject.send_now', true]] }]);
await page.waitForFunction(() => window.__values['inject.send_now'] === true);
assert.equal(await sendNow.getAttribute('aria-checked'), 'true');
await page.waitForTimeout(2600);
await page.screenshot({ path: `${out}/escuro-vibe-coding-ligado.png` });

// Subtela de frases: uma lista por idioma, grava a tabela inteira.
await page.getByRole('button', { name: 'Editar frases', exact: true }).click();
assert.equal(await page.getByRole('button', { name: 'Voltar aos terminais' }).evaluate(el => el === document.activeElement), true);
await page.getByRole('textbox', { name: 'Adicionar frase em português', exact: true }).fill('manda ver');
await page.getByRole('button', { name: 'Adicionar', exact: true }).first().click();
await page.waitForFunction(() => window.__values['inject.send_now_phrases'].pt.includes('manda ver'));
assert.deepEqual((await writes()).at(-1), ['settings_set', { changes: [['inject.send_now_phrases', { pt: ['enviar agora', 'manda ver'], en: ['send now'] }]] }]);
await page.getByRole('textbox', { name: 'Adicionar frase em inglês', exact: true }).fill('send now');
const beforeDuplicate = await writes();
await page.getByRole('button', { name: 'Adicionar', exact: true }).nth(1).click();
await page.getByText('Já está na lista.', { exact: true }).waitFor();
assert.deepEqual(await writes(), beforeDuplicate);
await page.getByRole('textbox', { name: 'Adicionar frase em inglês', exact: true }).fill('');
await page.waitForTimeout(2600);
await page.screenshot({ path: `${out}/escuro-frases.png` });
await page.getByRole('button', { name: 'Remover manda ver', exact: true }).click();
await page.waitForFunction(() => !window.__values['inject.send_now_phrases'].pt.includes('manda ver'));
assert.deepEqual((await writes()).at(-1), ['settings_set', { changes: [['inject.send_now_phrases', { pt: ['enviar agora'], en: ['send now'] }]] }]);
await page.getByRole('button', { name: 'Voltar aos terminais' }).click();
assert.equal(await page.getByRole('button', { name: 'Editar frases' }).evaluate(el => el === document.activeElement), true);

await sendNow.click();
await page.waitForFunction(() => window.__values['inject.send_now'] === false);
await setTheme('light');
await page.screenshot({ path: `${out}/claro-vibe-coding.png` });
await page.getByRole('button', { name: 'Editar frases', exact: true }).click();
await page.screenshot({ path: `${out}/claro-frases.png` });
await page.setViewportSize({ width: 600, height: 720 });
assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= 600), true);
await page.getByRole('button', { name: 'Voltar aos terminais' }).click();
assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= 600), true);

// Só as duas chaves novas (e o tema das capturas) foram gravadas.
const keys = new Set((await writes()).flatMap(([, args]) => args.changes.map(([key]) => key)));
assert.deepEqual([...keys].sort(), ['hub.theme', 'inject.send_now', 'inject.send_now_phrases']);
assert.deepEqual(errors, []);
fs.writeFileSync(`${out}/checks.json`, JSON.stringify({ url, viewport: '1100×720 (capturas) e 600×720 (overflow)', errors, checks: ['linha "Enter nunca é enviado" virou o switch', 'switch desligado por padrão e grava só inject.send_now', 'ajuda cita os limites (terminal, Enter, foco, texto comum)', 'resumo das frases na linha', 'subtela: uma lista por idioma, grava a tabela inteira', 'duplicada recusada sem gravar', 'foco na subtela e retorno', 'Escuro e Claro', 'sem overflow em 600×720', 'sem requisição externa nem erro de console'], calls: await page.evaluate(() => window.__calls.filter(([c]) => c === 'settings_set')) }, null, 2));
await browser.close();
console.log('E1 QA: passou, evidências em ' + out);
