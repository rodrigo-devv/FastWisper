// Ponte inteiramente falsa: só lê cores; não chama CLI/daemon nem grava a config do dono.
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ?? 'playwright');
import fs from 'node:fs';
import assert from 'node:assert/strict';
import os from 'node:os';
import path from 'node:path';
const out = process.env.EVIDENCE_DIR ?? '/tmp/fw-h2-evidence';
fs.mkdirSync(out, { recursive: true });
const fixture = {
  "ui.language": "pt-BR",
  'experimental.notes': false, 'hub.theme': 'auto', 'llm.enabled': true, 'llm.engine': 'ollama', 'llm.ollama_cloud': false,
  'llm.format_in_terminals': true, 'inject.terminal_apps': ['ghostty', 'kitty', 'Alacritty', 'foot'],
  'inject.harness_apps': [], 'inject.paste_apps': ['*'], 'inject.copy_only_apps': ['org.keepassxc.KeePassXC'], 'inject.notify': true
};
// Automático com paleta sintética: nada do tema desta máquina entra em QA ou capturas (A-04).
const colors={background:'#15171c',foreground:'#e8e6df',accent:'#5fb3a1',surface:'#1e2128'};
const browser = await chromium.launch({ headless: true, executablePath: process.env.BROWSER_BINARY || '/usr/bin/chromium' });
const context = await browser.newContext({ viewport: { width: 1100, height: 720 }, reducedMotion: 'reduce' });
await context.addInitScript(({ fixture, colors }) => {
  window.isTauri = true; window.__calls = []; window.__values = { ...fixture };
  let serial = 0; const callbacks = new Map(), listeners = new Map();
  window.__emit = (event, payload) => { for (const id of listeners.get(event) ?? []) callbacks.get(id)?.({ payload }); };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
  window.__TAURI_INTERNALS__ = {
    transformCallback: callback => { const id = ++serial; callbacks.set(id, callback); return id; },
    invoke: async (command, args = {}) => {
      window.__calls.push([command, args]);
      if (command === 'settings_get') return { values: { ...window.__values }, defaults: fixture, restart_keys: [] };
      if (command === 'settings_set') {
        if (window.__failNext) { window.__failNext = false; throw 'Falha simulada de gravação'; }
        for (const [key, value] of args.changes) window.__values[key] = value;
        return { status: 'applied', restart_required: [] };
      }
      if (command === 'hub_theme') return window.__values['hub.theme'];
      if (command === 'theme_colors') return colors;
      if (command === 'vram_total') return 8192;
      if (command === 'hub_version') return '0.1.0';
      // Destino da CLI colagem (o mapeamento é coberto no teste Rust do H0).
      if (command === 'initial_destination') return { kind: 'settings', tab: 'vibe-coding' };
      if (command === 'daemon_view') return { available: true, state: 'idle', hands_free: false, asr: 'ready', vram: null, last: null };
      if (command === 'notes_list') return [];
      if (command === 'history_stats') return { consecutive_weeks: 0, words: 0, ppm: 0 };
      if (command === 'plugin:event|listen') { const group = listeners.get(args.event) ?? []; group.push(args.handler); listeners.set(args.event, group); return ++serial; }
      if (command === 'plugin:event|unlisten') return null;
      throw new Error('Comando fora do mock: ' + command);
    }
  };
}, { fixture, colors });
const page = await context.newPage(); const errors = [];
page.on('pageerror', error => errors.push(error.message));
page.on('console', message => { if (['error', 'warning'].includes(message.type())) errors.push(message.text()); });
const url = process.env.HUB_URL || 'http://127.0.0.1:5194/';
await page.goto(url);
await page.getByRole('dialog', { name: 'Vibe coding', exact: true }).waitFor();
await page.getByRole('switch', { name: 'Formatar em terminais e harnesses', exact: true }).waitFor();
await page.evaluate(() => document.fonts.ready);
assert.equal(await page.title(), 'Início · FastWisper');
assert.equal(page.url(), url);
assert.equal(await page.locator('vite-error-overlay').count(), 0);
const writeCalls = () => page.evaluate(() => window.__calls.filter(([command]) => command === 'settings_set'));
async function assertWip(count) {
  const rows = page.locator('.adjustment-row').filter({ has: page.locator('.wip-badge') }).filter({ has: page.locator('button:disabled') });
  assert.equal(await rows.count(), count);
  const before = await writeCalls();
  for (const row of await rows.all()) {
    const control = row.locator('button');
    assert.equal(await control.isDisabled(), true);
    if (await control.getAttribute('role') === 'switch') assert.equal(await control.getAttribute('aria-checked'), 'false');
    // Cliques reais de mouse e teclado não ativam botões nativamente desabilitados.
    await control.scrollIntoViewIfNeeded();
    const box = await control.boundingBox();
    await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
    await page.keyboard.press('Space'); await page.keyboard.press('Enter');
  }
  assert.deepEqual(await writeCalls(), before);
}
await assertWip(3);
await page.getByRole('button', { name: 'Experimental', exact: true }).click();
await assertWip(3); // "Temas personalizados" saiu (A-11)
assert.equal(await page.locator('.adjustment-row').filter({ has: page.locator('.wip-badge') }).count(), 4);
const notesSwitch = page.getByRole('switch', { name: 'Notas por voz', exact: true });
assert.equal(await notesSwitch.isDisabled(), false);
assert.equal(await notesSwitch.getAttribute('aria-checked'), 'false');
await notesSwitch.click();
assert.deepEqual((await writeCalls()).at(-1), ['settings_set', { changes: [['experimental.notes', true]] }]);
await page.waitForFunction(() => window.__values['experimental.notes'] === true);
assert.equal(await notesSwitch.getAttribute('aria-checked'), 'true');
await notesSwitch.click();
await page.waitForFunction(() => window.__values['experimental.notes'] === false);
assert.deepEqual((await writeCalls()).at(-1), ['settings_set', { changes: [['experimental.notes', false]] }]);
// Link do composer da H3 abre a aba Experimental.
await page.getByRole('button', { name: 'Fechar', exact: true }).click();
await page.getByRole('button', { name: 'Configurações › Experimental' }).click();
await page.getByRole('dialog', { name: 'Experimental', exact: true }).waitFor();
await page.getByRole('button', { name: 'Vibe coding', exact: true }).click();
await page.getByRole('switch', { name: 'Formatar em terminais e harnesses', exact: true }).click();
assert.deepEqual((await writeCalls()).at(-1), ['settings_set', { changes: [['llm.format_in_terminals', false]] }]);
await page.getByRole('button', { name: 'Editar lista', exact: true }).click();
assert.equal(await page.getByRole('button', { name: 'Voltar aos terminais' }).evaluate(el => el === document.activeElement), true);
// inject.notify (página vibe-coding no catálogo) fica junto das listas de colagem.
await page.getByRole('switch', { name: 'Avisar quando só copiar', exact: true }).click();
assert.deepEqual((await writeCalls()).at(-1), ['settings_set', { changes: [['inject.notify', false]] }]);
const listKeys = ['inject.terminal_apps', 'inject.harness_apps', 'inject.paste_apps', 'inject.copy_only_apps'];
for (const [index, key] of listKeys.entries()) {
  const row = page.locator('.adjustment-row').filter({ has: page.locator(`#campo-${key.replace(/[^a-z_]/g, '-')}-label`) });
  await row.getByRole('textbox', { name: 'Adicionar item' }).fill(`fixture-app-${index}`);
  await row.getByRole('button', { name: 'Adicionar', exact: true }).click();
  await page.waitForFunction(({ key, index }) => window.__values[key].includes(`fixture-app-${index}`), { key, index });
  assert.equal((await writeCalls()).at(-1)[1].changes[0][0], key);
  await row.getByRole('button', { name: `Remover fixture-app-${index}`, exact: true }).click();
  await page.waitForFunction(({ key, index }) => !window.__values[key].includes(`fixture-app-${index}`), { key, index });
}
const terminalRow = page.locator('.adjustment-row').filter({ has: page.locator('#campo-inject-terminal_apps-label') });
await terminalRow.getByRole('textbox', { name: 'Adicionar item' }).fill('ghostty');
const beforeDuplicate = await writeCalls();
await terminalRow.getByRole('button', { name: 'Adicionar', exact: true }).click();
await page.getByText('Já está na lista.', { exact: true }).waitFor();
assert.deepEqual(await writeCalls(), beforeDuplicate);
await page.evaluate(() => window.__failNext = true);
await terminalRow.getByRole('textbox', { name: 'Adicionar item' }).fill('failed-app');
await terminalRow.getByRole('button', { name: 'Adicionar', exact: true }).click();
await page.getByText(/Não foi salvo: Falha simulada/).waitFor();
assert.equal(await terminalRow.getByRole('textbox').inputValue(), 'failed-app');
assert.equal(await page.evaluate(() => window.__values['inject.terminal_apps'].includes('failed-app')), false);
await page.getByRole('button', { name: 'Voltar aos terminais' }).click();
assert.equal(await page.getByRole('button', { name: 'Editar lista' }).evaluate(el => el === document.activeElement), true);
// Nova visita repõe os valores e remove toast/erros simulados antes das capturas.
await page.reload();
await page.getByRole('switch', { name: 'Formatar em terminais e harnesses' }).waitFor();
for (const theme of ['auto', 'dark', 'light']) {
  await page.getByRole('button', { name: 'Geral', exact: true }).click();
  await page.getByRole('radio', { name: { auto: 'Automático', dark: 'Escuro', light: 'Claro' }[theme], exact: true }).check();
  await page.waitForFunction(theme => document.documentElement.dataset.theme === theme, theme);
  await page.waitForTimeout(2600); // expira só o toast do mock
  for (const [tab, label] of [['vibe-coding', 'Vibe coding'], ['experimental', 'Experimental']]) {
    await page.getByRole('button', { name: label, exact: true }).click();
    await page.waitForTimeout(100);
    await page.screenshot({ path: `${out}/${theme}-${tab}.png` });
    const content = page.locator('.content.settings-modal');
    await content.evaluate(el => el.scrollTop = el.scrollHeight);
    await page.screenshot({ path: `${out}/${theme}-${tab}-scroll.png` });
    await content.evaluate(el => el.scrollTop = 0);
    // Coluna completa para as referências 23/24 (sem mudar largura nem medidas).
    await page.evaluate(() => {
      document.querySelector('.modal').style.height = 'auto';
      document.querySelector('.veil').style.position = 'absolute';
      document.querySelector('.veil').style.height = 'auto';
      document.querySelector('.veil').style.minHeight = '100vh';
      document.querySelector('.veil').style.alignItems = 'start';
      document.querySelector('.content.settings-modal').style.height = 'auto';
      document.querySelector('.content.settings-modal').style.overflow = 'visible';
    });
    await content.screenshot({ path: `${out}/${theme}-${tab}-full.png` });
    await page.evaluate(() => { for (const selector of ['.modal', '.veil', '.content.settings-modal']) document.querySelector(selector).removeAttribute('style'); });
  }
}
await page.getByRole('button', { name: 'Vibe coding', exact: true }).click();
await page.getByRole('button', { name: 'Editar lista', exact: true }).click();
await page.screenshot({ path: `${out}/light-list-editor.png` });
await page.setViewportSize({ width: 600, height: 720 });
await page.screenshot({ path: `${out}/mobile-list-editor.png` });
assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= 600), true);
await page.getByRole('button', { name: 'Voltar aos terminais' }).click();
await page.screenshot({ path: `${out}/mobile-vibe-coding.png` });
await page.getByRole('button', { name: 'Experimental', exact: true }).click();
await page.screenshot({ path: `${out}/mobile-experimental.png` });
assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= 600), true);
assert.deepEqual(errors, []);
fs.writeFileSync(`${out}/checks.json`, JSON.stringify({ url, viewport: '1100×720 e 600×720', colors, errors, checks: ['colagem → Vibe coding', 'WIP sem escritas por mouse/teclado', 'Notas com selo WIP e chave opt-in ativa', 'composer → Experimental', 'formatar em terminais', 'três ListEditors: adicionar/remover/duplicado/falha', 'foco na subtela e retorno', 'três temas e rolagem', 'sem overflow horizontal'], calls: await page.evaluate(() => window.__calls) }, null, 2));
await browser.close();
console.log('H2 QA: passou, evidências em ' + out);
