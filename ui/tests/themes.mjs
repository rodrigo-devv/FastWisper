// QA de UI com ponte Tauri inteiramente falsa: nunca chama a CLI, o daemon ou o Ollama.
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ?? 'playwright');
import fs from 'node:fs';
import assert from 'node:assert/strict';
import os from 'node:os';
import path from 'node:path';
const out=process.env.EVIDENCE_DIR ?? '/tmp/fw-h6-evidence'; fs.mkdirSync(out,{recursive:true});
const fixture = {
  "ui.language": "pt-BR",
 'hub.theme':'auto','audio.device':'fixture.microphone','asr.languages':['pt','en'],'hotkey.max_recording_secs':300,
 'audio.sounds':true,'audio.sound_volume':.4,'overlay.enabled':true,'overlay.position':'bottom',
 'asr.model':'/fixture/ggml-small.bin','asr.gpu':'auto','llm.enabled':true,'llm.model':'qwen3:4b-instruct',
 'llm.style':'casual','llm.timeout_ms':2000,'llm.skip_below_words':3,'llm.format_in_terminals':false,
 'performance.vram':'automatico','performance.unload_after_secs':300,'performance.game_llm':'local',
 'llm.ollama_cloud':false,'inject.clipboard_history':false,'llm.engine':'ollama','llm.endpoint':'http://127.0.0.1:11434',
 'llm.remote_consent':false,'llm.api_key_file':'','inject.paste_apps':['*'],'inject.copy_only_apps':[],
 'inject.terminal_apps':['ghostty'],'inject.notify':true,'dictionary.words':[],'dictionary.replace':{},'snippets':[]
};
// Automático com paleta sintética: nada do tema desta máquina entra em QA ou capturas (A-04).
const colors={background:'#15171c',foreground:'#e8e6df',accent:'#5fb3a1',surface:'#1e2128'};
// Catálogo só com os temas de fábrica do Omarchy (públicos), lidos no momento
// da QA; temas pessoais de ~/.config/omarchy/themes nunca entram (A-04).
const themes = new Map();
for (const root of [process.env.OMARCHY_THEMES_DIR || '/usr/share/omarchy/themes']) {
 if (!fs.existsSync(root)) continue;
 for (const name of fs.readdirSync(root).sort()) {
  if (!/^[a-zA-Z0-9_-]{1,128}$/.test(name)) continue;
  const file = path.join(root, name, 'colors.toml');
  themes.delete(name);
  if (!fs.existsSync(file)) continue;
  const text = fs.readFileSync(file, 'utf8');
  const read = key => text.match(new RegExp(`^${key}\\s*=\\s*"(#[0-9a-fA-F]{6})"`, 'm'))?.[1] ?? null;
  const palette = { background: read('background'), foreground: read('foreground'), accent: read('accent'), surface: read('lighter_background') };
  if (palette.background && palette.foreground && palette.accent) themes.set(name, { name, colors: palette });
 }
}
const catalog = [...themes.values()].sort((a,b) => a.name.localeCompare(b.name));
const browser=await chromium.launch({headless:true,executablePath:process.env.BROWSER_BINARY || '/usr/bin/chromium'});
const context=await browser.newContext({viewport:{width:1100,height:720},reducedMotion:'reduce'});
await context.addInitScript(({fixture,colors,catalog})=>{
 window.__themes=catalog; window.isTauri=true; window.__calls=[]; window.__values={...fixture};
 const vram={mode:'automatico',profile:'sob_demanda',game:false,asr:'unloaded',llm:'unloaded',vram_mb:0,unload_in_secs:null};
 let serial=0; const callbacks=new Map(), listeners=new Map();
 window.__emit=(event,payload)=>{ for(const id of listeners.get(event) ?? []) callbacks.get(id)?.({payload}); };
 window.__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener(){}};
 window.__TAURI_INTERNALS__={transformCallback:callback=>{ const id=++serial; callbacks.set(id,callback); return id; }, invoke:async(cmd,args={})=>{
   window.__calls.push([cmd,args]);
   const values=window.__values;
   if(cmd==='settings_get') { await new Promise(resolve=>setTimeout(resolve,50)); return {values:{...values},defaults:fixture,restart_keys:['asr.model','asr.gpu','asr.threads','hotkey.min_recording_ms','llm.endpoint','llm.engine','llm.remote_consent']}; }
   if(cmd==='settings_set') { if(window.__failNext) { window.__failNext=false; throw 'Falha simulada de gravação'; } for(const [key,value] of args.changes) values[key]=value; return {status:'applied',restart_required:args.changes.map(x=>x[0]).filter(x=>['asr.model','asr.gpu','asr.threads','hotkey.min_recording_ms','llm.endpoint','llm.engine','llm.remote_consent'].includes(x))}; }
   if(cmd==='ui_language') return 'pt-BR';
    if(cmd==='hub_theme') return values['hub.theme'];
   if(cmd==='theme_colors') return args.name ? window.__themes.find(theme=>theme.name===args.name)?.colors ?? null : colors;
   if(cmd==='omarchy_themes') return window.__themes;
   if(cmd==='hub_version') return '0.1.0';
   if(cmd==='initial_destination') return null;
   if(cmd==='daemon_view') return {available:true,state:'idle',hands_free:false,asr:'ready',vram,last:null};
   if(cmd==='vram_total') return 8192;
   if(cmd==='doctor_summary') return {verdict:'Há um aviso',ok:8,notices:[{item:'Modelo da IA',level:'warn',detail:'Modelo fora do manifesto recomendado.',hint:'Confira o modelo escolhido.'}]};
   if(cmd==='microphones') return [{node_name:'fixture.microphone',description:'Microfone padrão',chosen:true},{node_name:'fixture.usb',description:'Microfone USB',chosen:false}];
   if(cmd==='asr_models') return [{id:'small',size_mb:466,note:'Multilíngue',license:'MIT',path:'/fixture/ggml-small.bin',installed:true,active:true},{id:'medium',size_mb:1533,note:'Multilíngue',license:'MIT',path:'/fixture/ggml-medium.bin',installed:!!window.__mediumInstalled,active:values['asr.model']==='/fixture/ggml-medium.bin'}];
   if(cmd==='asr_gpus') return ['GPU Vulkan de exemplo'];
   if(cmd==='ollama_models') return ['qwen3:4b-instruct','qwen3:8b'];
   if(cmd==='ollama_cloud') return {cloud:values['llm.ollama_cloud'],disabled:!values['llm.ollama_cloud'],error:null};
   if(cmd==='ollama_cloud_set') { values['llm.ollama_cloud']=args.online; return {cloud:args.online,disabled:!args.online,error:null}; }
   if(cmd==='mic_test') return .45;
   if(cmd==='history_stats') return {days:args.days,consecutive_weeks:window.__cleared?0:1,words:window.__cleared?0:window.__singleWord?1:123,ppm:window.__cleared?0:82,empty:false,total:window.__cleared?0:4,outcomes:window.__cleared?[]:[{name:'transcribed',count:4}],deliveries:[],profiles:[],apps:[],asr_ms:{count:0,p50:0,p95:0,max:0},llm_ms:{count:0,p50:0,p95:0,max:0},e2e_ms:{count:0,p50:0,p95:0,max:0},load_ms:{count:0,p50:0,p95:0,max:0},llm_load_ms:{count:0,p50:0,p95:0,max:0}};
   if(cmd==='history_clear') { window.__cleared=true; window.__emit('history-cleared',null); return 4; }
   if(cmd==='plugin:event|listen') { const group=listeners.get(args.event)??[]; group.push(args.handler); listeners.set(args.event,group); return ++serial; }
   if(cmd==='plugin:event|unlisten') return null;
   if(cmd==='asr_model_pull') { const handler=callbacks.get(listeners.get('model-progress')?.at(-1)); handler?.({payload:{id:args.id,progress:40}}); await new Promise(resolve=>setTimeout(resolve,150)); window.__mediumInstalled=true; return null; }
   if(['meter_start','meter_stop','sound_test','daemon_restart','open_guide','release_vram','asr_model_pull'].includes(cmd)) return null;
   throw new Error('Unhandled mock: '+cmd);
 }};
},{fixture,colors,catalog});
const page=await context.newPage(); const errors=[]; const external=[];
page.on('pageerror',e=>errors.push(e.message));
page.on('console',m=>{if(['error','warning'].includes(m.type())) errors.push(m.text());});
page.on('request',req=>{ if (!new URL(req.url()).hostname.match(/^(127\.0\.0\.1|localhost)$/)) external.push(req.url()); });
const url = process.env.HUB_URL || 'http://127.0.0.1:5196/';
await page.goto(url);
await page.getByRole('link',{name:'Temas',exact:true}).click();
await page.getByRole('heading',{name:'Temas',exact:true}).waitFor();
await page.getByRole('radio',{name:'Automático',exact:true}).waitFor({state:'visible'});
await page.waitForFunction(()=>!document.querySelector('.card')?.disabled);
await page.evaluate(()=>document.fonts.ready);
assert.equal(new URL(page.url()).pathname,'/temas');
assert.equal(await page.title(),'Temas · FastWisper');
assert.equal(await page.getByRole('radio').count(),3+catalog.length);
assert.equal(await page.getByRole('link',{name:'Temas',exact:true}).getAttribute('aria-current'),'page');
const choose = async name => {
 await page.getByRole('radio',{name,exact:true}).click();
 await page.waitForFunction(()=>!document.querySelector('.card')?.disabled);
};
const capture = async name => { await page.screenshot({path:`${out}/${name}.png`}); };
for (const [mode,label] of [['auto','Automático'],['dark','Escuro'],['light','Claro']]) {
 await choose(label);
 await page.waitForFunction(mode=>document.documentElement.dataset.theme===mode,mode);
 await capture(`galeria-${mode}`);
 await page.getByRole('link',{name:'Início',exact:true}).click();
 await page.getByRole('heading',{name:'Bem-vindo de volta',exact:true}).waitFor();
 await capture(`inicio-${mode}`);
 await page.getByRole('link',{name:'Temas',exact:true}).click();
}
const applied = ['tokyo-night','catppuccin-latte','gruvbox'].filter(name=>themes.has(name));
for (const name of applied) {
 await choose(name);
 await page.waitForFunction(name=>document.documentElement.dataset.theme===`omarchy:${name}`,name);
 const palette = themes.get(name).colors;
 const saved = await page.evaluate(()=>window.__values['hub.theme']);
 assert.equal(saved,`omarchy:${name}`);
 assert.equal(await page.getByRole('radio',{name,exact:true}).getAttribute('aria-checked'),'true');
 assert.equal(await page.locator('.card.chosen').evaluate(el=>getComputedStyle(el).borderTopWidth),'2px');
 const rendered = await page.evaluate(()=>({bg:document.documentElement.style.getPropertyValue('--bg'),fg:document.documentElement.style.getPropertyValue('--text'),accent:document.documentElement.style.getPropertyValue('--accent')}));
 assert.deepEqual(rendered,{bg:palette.background,fg:palette.foreground,accent:palette.accent});
 await page.locator('.shell > main').evaluate(el=>el.scrollTop=0);
 await capture(`galeria-${name}`);
 const selected = page.getByRole('radio',{name,exact:true});
 await selected.scrollIntoViewIfNeeded(); await capture(`selecionado-${name}`);
 await page.getByRole('link',{name:'Início',exact:true}).click();
 await page.getByRole('heading',{name:'Bem-vindo de volta',exact:true}).waitFor();
 await capture(`inicio-${name}`);
 await page.getByRole('button',{name:'Configurações',exact:true}).click();
 await page.getByText(`Tema do Omarchy selecionado:`,{exact:false}).waitFor();
 assert.equal(await page.locator('.modal').getByRole('radio').count(),3);
 assert.equal(await page.locator('.modal input:checked[name=hub-theme]').count(),0);
 if(name===applied[0]) await capture('geral-omarchy');
 await page.getByRole('link',{name:'Abrir galeria de temas',exact:true}).click();
 await page.getByRole('heading',{name:'Temas',exact:true}).waitFor();
 assert.equal(await page.locator('.modal').count(),0);
}
// Falha de gravação mantém a seleção e as cores persistidas.
const before = await page.evaluate(()=>window.__values['hub.theme']);
await page.evaluate(()=>window.__failNext=true);
await choose('Escuro');
assert.equal(await page.evaluate(()=>window.__values['hub.theme']),before);
assert.equal(await page.getByRole('radio',{name:before.startsWith('omarchy:')?before.slice(8):'Claro',exact:true}).getAttribute('aria-checked'),'true');
// Config alterada pela CLI, tema removido, recuperado e sem Omarchy.
const missing='removed-theme';
await page.evaluate(name=>{window.__values['hub.theme']=`omarchy:${name}`;window.__emit('theme-changed');},missing);
await page.getByRole('status').filter({hasText:'não está disponível'}).waitFor();
await page.waitForFunction(()=>document.documentElement.dataset.theme==='auto');
await capture('tema-ausente');
if(applied.length) {
 const recovered=themes.get(applied[0]);
 await page.evaluate(theme=>{window.__themes.push({...theme,name:'removed-theme'});window.__emit('theme-changed');},recovered);
 await page.waitForFunction(()=>document.documentElement.dataset.theme==='omarchy:removed-theme');
 assert.equal(await page.getByRole('status').filter({hasText:'não está disponível'}).count(),0);
}
await page.evaluate(()=>{window.__themes=[];window.__values['hub.theme']='auto';window.__emit('theme-changed');});
await page.waitForFunction(()=>document.querySelectorAll('[role=radio]').length===3);
await capture('sem-omarchy');
await page.setViewportSize({width:600,height:720});
assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=600),true);
await capture('galeria-mobile');
await page.setViewportSize({width:1100,height:720});
// Teclado: cartões alcançáveis, Enter aplica; reduced-motion ativo na QA.
await page.getByRole('radio',{name:'Escuro',exact:true}).focus();
await page.keyboard.press('Enter');
await page.waitForFunction(()=>document.documentElement.dataset.theme==='dark');
await page.keyboard.press('ArrowRight');
await page.waitForFunction(()=>document.documentElement.dataset.theme==='light');
assert.equal(await page.getByRole('radio',{name:'Claro',exact:true}).evaluate(el=>el===document.activeElement),true);
assert.deepEqual(errors,[]); assert.deepEqual(external,[]);
fs.writeFileSync(`${out}/checks.json`,JSON.stringify({url:page.url(),title:await page.title(),viewport:[1100,720],catalog:catalog.map(theme=>theme.name),applied,errors,external,checks:['rota e menu ativo','três modos ao vivo','cores do hub iguais ao colors.toml','cartão selecionado com borda 2px','Geral informa tema instalado e link volta à galeria','falha ao salvar preserva escolha','config externa acompanha evento','remoção cai em Automático com aviso','reaparecimento restaura tema','sem Omarchy só três modos','teclado Enter e setas','sem overflow em 600x720','sem erros de console ou rede externa'],calls:await page.evaluate(()=>window.__calls)},null,2));
await browser.close(); console.log(`QA passed: ${out}; ${catalog.length} temas, ${applied.length} aplicados`);
