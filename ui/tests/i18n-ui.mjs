// QA dos atalhos editáveis (E2) com ponte Tauri inteiramente falsa: nunca
// chama a CLI, o daemon, o hyprctl nem o Hyprland. Gera as capturas de aceite.
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ?? 'playwright');
import fs from 'node:fs';
import assert from 'node:assert/strict';
const out = process.env.EVIDENCE_DIR ?? '/tmp/fw-e3-evidence'; fs.mkdirSync(out, { recursive: true });
const fixture = {
  
  'hub.theme': 'dark', 'audio.device': 'fixture.microphone', 'asr.languages': ['pt', 'en'], 'hotkey.max_recording_secs': 300,
  'hotkey.dictate': 'ALT+SPACE', 'hotkey.hands_free': 'ALT+SHIFT+SPACE', 'hotkey.paste_last': 'SUPER+ALT+V',
  'audio.sounds': true, 'audio.sound_volume': .4, 'overlay.enabled': true, 'overlay.position': 'bottom',
  'asr.model': '/fixture/ggml-small.bin', 'asr.gpu': 'auto', 'llm.enabled': true, 'llm.model': 'qwen3:4b-instruct',
  'llm.style': 'casual', 'llm.timeout_ms': 2000, 'llm.skip_below_words': 3, 'llm.format_in_terminals': true,
  'performance.vram': 'automatico', 'performance.unload_after_secs': 300, 'performance.game_llm': 'local',
  'llm.ollama_cloud': false, 'inject.clipboard_history': false, 'llm.engine': 'ollama', 'llm.endpoint': 'http://127.0.0.1:11434',
  'llm.remote_consent': false, 'llm.api_key_file': '', 'inject.paste_apps': ['*'], 'inject.copy_only_apps': [],
  'inject.harness_apps': [], 'inject.terminal_apps': ['ghostty'], 'inject.notify': true, 'dictionary.words': [], 'dictionary.replace': {}, 'snippets': [],
  'experimental.notes': true, 'inject.send_now': false, 'inject.send_now_phrases': { pt: ['enviar agora'], en: ['send now'] }
};
// Recorte da tabela do fw_core (nome, keycode, KeyboardEvent.code, rótulo).
const keys = [
  ['D', 40, 'KeyD', 'D'], ['V', 55, 'KeyV', 'V'], ['F9', 75, 'F9', 'F9'], ['SPACE', 65, 'Space', 'Espaço'], ['RETURN', 36, 'Enter', 'Enter']
].map(([name, keycode, code, label]) => ({ name, keycode, code, label }));
const browser = await chromium.launch({ headless: true, executablePath: process.env.BROWSER_BINARY || '/usr/bin/chromium' });
const context = await browser.newContext({ viewport: { width: 1100, height: 720 }, reducedMotion: 'reduce' });
await context.addInitScript(({ fixture, keys }) => {
  window.isTauri = true; window.__calls = []; window.__unhandled = []; window.__values = { ...fixture };
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
    if (cmd === 'ui_language') {
      const language = values['ui.language'] ?? 'en';
      if (window.__delay_language_read) return new Promise(resolve => { window.__finish_language_read = () => resolve(language); });
      return language;
    }
    if (cmd === 'hub_theme') return values['hub.theme'];
    if (cmd === 'theme_colors') return { background: null, foreground: null, accent: null, surface: null };
    if (cmd === 'hub_version') return '0.1.0';
    if (cmd === 'initial_destination') return null;
    if (cmd === 'daemon_view') return { available: true, state: window.__recording ? 'recording' : 'idle', hands_free: false, note_session: false, asr: 'ready', vram, last: null };
    if (cmd === 'vram_total') return 16384;
    if (cmd === 'microphones') return [{ node_name: 'fixture.microphone', description: 'Microfone padrão', chosen: true }];
    if (cmd === 'history_stats') return { days: args.days, consecutive_weeks: 1, words: 123, ppm: 82, empty: false, total: 4, outcomes: [], deliveries: [], profiles: [], apps: [], asr_ms: { count: 0, p50: 0, p95: 0, max: 0 }, llm_ms: { count: 0, p50: 0, p95: 0, max: 0 }, e2e_ms: { count: 0, p50: 0, p95: 0, max: 0 }, load_ms: { count: 0, p50: 0, p95: 0, max: 0 }, llm_load_ms: { count: 0, p50: 0, p95: 0, max: 0 } };
    if (cmd === 'notes_list') return [{ id: 1, ts: Math.floor(Date.now()/1000), text: 'Synthetic note: português stays unchanged', language: 'pt', words: 6 }];
    if (cmd === 'doctor_summary') return {verdict:'Tudo certo.',ok:8,notices:[]};
    if (cmd === 'asr_models') return [];
    if (cmd === 'asr_gpus') return ['Fixture GPU'];
    if (cmd === 'ollama_models') return ['qwen3:4b-instruct'];
    if (cmd === 'ollama_cloud') return {cloud:false,disabled:true,error:null};
    if (cmd === 'omarchy_themes') return []; 
    if (cmd === 'plugin:event|listen') { const group = listeners.get(args.event) ?? []; group.push(args.handler); listeners.set(args.event, group); return ++serial; }
    if (cmd === 'plugin:event|unlisten') return null;
    if (['meter_start', 'meter_stop', 'sound_test', 'mic_test'].includes(cmd)) return null;
    window.__unhandled.push(cmd); throw new Error('Unhandled mock: ' + cmd);
  } };
}, { fixture, keys });
const page = await context.newPage(); const errors = [], network = [];
page.on('pageerror', (e) => errors.push(e.message));
page.on('console', (m) => { if (['error', 'warning'].includes(m.type())) errors.push(m.text()); });
const url = process.env.HUB_URL || 'http://127.0.0.1:5198/';
page.on('request', (r) => { if (!r.url().startsWith(new URL(url).origin)) network.push(r.url()); });
await page.goto(url);
await page.getByRole('button', { name: 'Settings', exact: true }).waitFor();
await page.evaluate(() => document.fonts.ready);

assert.equal(await page.title(), 'Home · FastWisper');
await page.getByText('Welcome back', {exact:true}).waitFor();
assert.equal(await page.evaluate(() => document.documentElement.lang), 'en');
const originalLanguages = await page.evaluate(() => window.__values['asr.languages']);
const originalPhrases = await page.evaluate(() => window.__values['inject.send_now_phrases']);
const shot = async (name) => {
 await page.evaluate(() => document.fonts.ready);
 await page.waitForTimeout(120);
 await page.screenshot({path:`${out}/${name}.png`});
 // E4: mesmo estado em Claro, só na ponte falsa; mantém Escuro para o próximo passo.
 if (process.env.CAPTURE_LIGHT_SCREENS === '1' && name.endsWith('-dark')) {
   await page.evaluate(() => { window.__values['hub.theme'] = 'light'; window.__emit('theme-changed', null); });
   await page.waitForFunction(() => document.documentElement.dataset.theme === 'light');
   await page.waitForTimeout(120);
   await page.screenshot({path:`${out}/${name.replace(/-dark$/, '-light')}.png`});
   await page.evaluate(() => { window.__values['hub.theme'] = 'dark'; window.__emit('theme-changed', null); });
   await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark');
 }
};
const names = {
 en: {settings:'Settings',general:'General',system:'System',vibe:'Vibe coding',experimental:'Experimental',privacy:'Data and privacy',help:'Help',about:'About',language:'FastWisper language',history:'History',themes:'Themes'},
 'pt-BR': {settings:'Configurações',general:'Geral',system:'Sistema',vibe:'Vibe coding',experimental:'Experimental',privacy:'Dados e privacidade',help:'Ajuda',about:'Sobre',language:'Idioma do FastWisper',history:'Histórico',themes:'Temas'}
};
for (const language of ['en','pt-BR']) {
 const n=names[language];
 await page.getByRole('button',{name:language==='en'?'Settings':'Settings',exact:true}).click();
 if (language==='pt-BR') {
   await page.getByLabel('Maximum duration',{exact:true}).fill('301');
   await page.getByLabel('FastWisper language',{exact:true}).focus();
   await page.getByRole('alert').filter({hasText:'Use a value from 1 to 300.'}).waitFor();
   await page.getByLabel('FastWisper language',{exact:true}).selectOption('pt-BR');
   await page.getByRole('dialog').getByRole('button',{name:'Geral',exact:true}).waitFor();
   await page.getByRole('alert').filter({hasText:'Use de 1 a 300.'}).waitFor();
   await page.getByLabel('Duração máxima',{exact:true}).fill('300');
   await page.getByLabel('Idioma do FastWisper',{exact:true}).focus();
 }
 assert.equal(await page.evaluate(() => document.documentElement.lang),language);
 const select = page.getByLabel(n.language,{exact:true});
 assert.equal(await select.inputValue(),language);
 assert.deepEqual(await page.evaluate(() => window.__values['asr.languages']),originalLanguages);
 assert.deepEqual(await page.evaluate(() => window.__values['inject.send_now_phrases']),originalPhrases);
 // Fonte de nomes de idioma mantém a autodenominação em ambas as interfaces.
 assert.deepEqual(await select.locator('option').allTextContents(),['English','Português (Brasil)']);
 for(const [name,title] of [['geral',n.general],['sistema',n.system],['vibe-coding',n.vibe],['experimental',n.experimental],['privacidade',n.privacy],['ajuda',n.help],['sobre',n.about]]) {
   await page.getByRole('dialog').getByRole('button',{name:title,exact:true}).click();
   await page.getByRole('dialog').getByRole('heading',{name:title,exact:true}).waitFor();
   await shot(`${language}-${name}-dark`);
   if (name === 'geral') {
     // Geral › Ditado a partir do título do grupo: as três linhas de atalho.
     const dictation = page.getByRole('dialog').getByRole('region',{name:language === 'en' ? 'Dictation' : 'Ditado',exact:true});
     await dictation.evaluate(section => {
       section.scrollIntoView({block:'start'});
       let box = section.parentElement;
       while (box && box.scrollHeight <= box.clientHeight) box = box.parentElement;
       box?.scrollBy(0, -24); // título do grupo inteiro na captura
     });
     await shot(`${language}-geral-ditado-dark`);
   }
 }
 await page.keyboard.press('Escape');
 await page.getByRole('dialog').waitFor({state:'detached'});
 await shot(`${language}-inicio-dark`);
 for(const [route,name,title] of [['/historico','historico',n.history],['/temas','temas',n.themes],['/personalizacao/dicionario','dicionario',language==='en'?'Dictionary':'Dicionário'],['/personalizacao/snippets','snippets','Snippets'],['/personalizacao/estilo','estilo',language==='en'?'Style':'Estilo'],['/notas','notas',language==='en'?'Notes':'Notas']]) {
   await page.locator(`.sidebar a[href="${route}"]`).click();
   await page.getByRole('heading',{name:title,exact:true}).waitFor();
   await shot(`${language}-${name}-dark`);
   if (name === 'geral') {
     await page.getByRole('group',{name:language === 'en' ? 'Languages' : 'Idiomas',exact:true}).scrollIntoViewIfNeeded();
     await shot(`${language}-geral-ditado-dark`);
   }
 }
 await page.locator('.brand').click();
 await page.getByRole('button',{name:n.settings,exact:true}).click();
 await page.getByRole('dialog').getByRole('button',{name:n.general,exact:true}).click();
 const light=page.getByRole('radio',{name:language==='en'?'Light':'Claro',exact:true});
 await light.check();
 await page.evaluate(()=>window.__emit('theme-changed',null));
 await page.waitForFunction(()=>document.documentElement.dataset.theme==='light');
 await shot(`${language}-geral-light`);
 await page.getByRole('radio',{name:language==='en'?'Dark':'Escuro',exact:true}).check();
 await page.evaluate(()=>window.__emit('theme-changed',null));
 await page.waitForFunction(()=>document.documentElement.dataset.theme==='dark');
 await page.keyboard.press('Escape');
}
await page.getByRole('button',{name:'Configurações',exact:true}).click();
await page.getByRole('dialog').getByRole('button',{name:'Geral',exact:true}).click();
assert.equal(await page.getByLabel('Idioma do FastWisper',{exact:true}).inputValue(),'pt-BR');
// Uma leitura antiga não pode desfazer a escolha mais recente do seletor.
await page.evaluate(()=>{window.__delay_language_read=true;window.__emit('theme-changed',null);});
await page.waitForFunction(()=>typeof window.__finish_language_read==='function');
await page.getByLabel('Idioma do FastWisper',{exact:true}).selectOption('en');
await page.evaluate(()=>{window.__delay_language_read=false;window.__finish_language_read();});
assert.equal(await page.getByLabel('FastWisper language',{exact:true}).inputValue(),'en');
await page.getByLabel('FastWisper language',{exact:true}).selectOption('pt-BR');
// Mudança externa da CLI: o evento do observador atualiza o idioma no hub sem reload.
await page.evaluate(()=>{window.__values['ui.language']='en';window.__emit('theme-changed',null);});
await page.getByRole('dialog').getByRole('heading',{name:'General',exact:true}).waitFor();
assert.equal(await page.getByLabel('FastWisper language',{exact:true}).inputValue(),'en');
await page.keyboard.press('Escape');
await page.getByText('Welcome back',{exact:true}).waitFor();
await page.setViewportSize({width:680,height:720});
await page.getByRole('button',{name:'Settings',exact:true}).click();
await page.getByLabel('FastWisper language',{exact:true}).waitFor();
assert.equal(await page.locator('vite-error-overlay').count(),0);
assert.deepEqual(await page.evaluate(()=>window.__unhandled),[]);
assert.deepEqual(errors,[]);
assert.deepEqual(network,[]);
fs.writeFileSync(`${out}/checks.json`,JSON.stringify({url,viewports:[[1100,720],[680,720]],browser:'Playwright/Chromium',browserPlugin:'Browser plugin not available',bridge:'Tauri falsa; nenhum daemon/áudio/Hyprland real',checks:['inglês sem chave','troca ao vivo via select e evento externo', 'resposta antiga não desfaz escolha nova', 'seletor aberto acompanha idioma externo','catálogo no idioma novo', 'idiomas da transcrição e descrições separados da interface','asr.languages e send_now_phrases intactos','nomes nativos de idiomas','telas principais em Escuro e Geral em Claro','conteúdo opt-in de nota preservado','sem rede externa, overlay ou erros de console'],errors,network},null,2)+'\n');
await browser.close();
console.log('i18n-ui: PASS');
