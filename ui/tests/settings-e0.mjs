// QA de UI com ponte Tauri inteiramente falsa: nunca chama a CLI, o daemon ou o Ollama.
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ?? 'playwright');
import fs from 'node:fs';
import assert from 'node:assert/strict';
import os from 'node:os';
import path from 'node:path';
const out=process.env.EVIDENCE_DIR ?? '/tmp/fw-e0-evidence'; fs.mkdirSync(out,{recursive:true});
const fixture = {
  "ui.language": "pt-BR",
 'hub.theme':'auto','audio.device':'fixture.microphone','asr.languages':['pt','en'],'hotkey.max_recording_secs':300,
 'audio.sounds':true,'audio.sound_volume':.4,'overlay.enabled':true,'overlay.position':'bottom',
 'asr.model':'/fixture/ggml-small.bin','asr.gpu':'auto','llm.enabled':true,'llm.model':'qwen3:4b-instruct',
 'llm.style':'casual','llm.timeout_ms':2000,'llm.skip_below_words':3,'llm.format_in_terminals':true,
 'performance.vram':'automatico','performance.unload_after_secs':300,'performance.game_llm':'local',
 'llm.ollama_cloud':false,'inject.clipboard_history':false,'llm.engine':'ollama','llm.endpoint':'http://127.0.0.1:11434',
 'llm.remote_consent':false,'llm.api_key_file':'','inject.paste_apps':['*'],'inject.copy_only_apps':[],
 'inject.harness_apps':[],'inject.terminal_apps':['ghostty'],'inject.notify':true,'dictionary.words':[],'dictionary.replace':{},'snippets':[]
};
// Automático com paleta sintética: nada do tema desta máquina entra em QA ou capturas (A-04).
const colors={background:'#15171c',foreground:'#e8e6df',accent:'#5fb3a1',surface:'#1e2128'};
const browser=await chromium.launch({headless:true,executablePath:process.env.BROWSER_BINARY || '/usr/bin/chromium'});
const context=await browser.newContext({viewport:{width:1100,height:720},reducedMotion:'reduce'});
await context.addInitScript(({fixture,colors})=>{
 window.isTauri=true; window.__calls=[]; window.__values={...fixture};
 const vram={mode:'automatico',profile:'sob_demanda',game:false,asr:'unloaded',llm:'unloaded',vram_mb:0,unload_in_secs:null};
 let serial=0; const callbacks=new Map(), listeners=new Map();
 window.__emit=(event,payload)=>{ for(const id of listeners.get(event) ?? []) callbacks.get(id)?.({payload}); };
 window.__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener(){}};
 window.__TAURI_INTERNALS__={transformCallback:callback=>{ const id=++serial; callbacks.set(id,callback); return id; }, invoke:async(cmd,args={})=>{
   window.__calls.push([cmd,args]);
   const values=window.__values;
   if(cmd==='settings_get' && window.__failSettings) throw 'Falha simulada de leitura';
   if(cmd==='settings_get') { await new Promise(resolve=>setTimeout(resolve,50)); return {values:{...values},defaults:fixture,restart_keys:['asr.model','asr.gpu','asr.threads','hotkey.min_recording_ms','llm.endpoint','llm.engine','llm.remote_consent']}; }
   if(cmd==='settings_set') { if(window.__failNext) { window.__failNext=false; throw 'Falha simulada de gravação'; } for(const [key,value] of args.changes) values[key]=value; return {status:'applied',restart_required:args.changes.map(x=>x[0]).filter(x=>['asr.model','asr.gpu','asr.threads','hotkey.min_recording_ms','llm.endpoint','llm.engine','llm.remote_consent'].includes(x))}; }
   if(cmd==='ui_language') return 'pt-BR';
    if(cmd==='hub_theme') return values['hub.theme'];
   if(cmd==='theme_colors') return colors;
   if(cmd==='hub_version') return '0.1.0';
   if(cmd==='initial_destination') return window.__initialDestination ?? null;
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
   if(cmd==='open_about_link') return null;
   if(['meter_start','meter_stop','sound_test','daemon_restart','open_guide','release_vram','asr_model_pull'].includes(cmd)) return null;
   throw new Error('Unhandled mock: '+cmd);
 }};
},{fixture,colors});
const page=await context.newPage(); const errors=[], network=[];
page.on('pageerror',e=>errors.push(e.message));
page.on('console',m=>{if(['error','warning'].includes(m.type())) errors.push(m.text());});
const url=process.env.HUB_URL || 'http://127.0.0.1:5196/';
page.on('request',r=>{if(!r.url().startsWith(new URL(url).origin)) network.push(r.url());});
await page.goto(url);
await page.getByRole('button',{name:'Configurações',exact:true}).waitFor();
assert.equal(page.url(),url);
assert.equal(await page.title(),'Início · FastWisper');
assert.equal(await page.locator('.sidebar').getByRole('button',{name:'Ajuda',exact:true}).count(),0);
await page.getByRole('button',{name:'Configurações',exact:true}).click();
await page.getByRole('dialog',{name:'Geral',exact:true}).waitFor();
await page.evaluate(()=>document.fonts.ready);
const menu=page.getByRole('navigation',{name:'Abas de Configurações'});
assert.deepEqual(await menu.getByRole('button').allTextContents(), ['Geral','Sistema','Vibe coding','Experimental','Dados e privacidade','Ajuda','Sobre']);
for(const theme of ['dark','light']) {
 await menu.getByRole('button',{name:'Geral',exact:true}).click();
 await page.getByRole('radio',{name:theme==='dark'?'Escuro':'Claro',exact:true}).check();
 await page.waitForFunction(theme=>document.documentElement.dataset.theme===theme,theme);
 await page.waitForTimeout(2600);
 await page.screenshot({path:`${out}/${theme}-menu.png`});
 await menu.getByRole('button',{name:'Sobre',exact:true}).click();
 await page.getByRole('dialog',{name:'Sobre',exact:true}).waitFor();
 assert.match(await page.getByRole('dialog').innerText(),/100% local.*Omarchy\/Hyprland/);
 await page.getByText('v0.1.0',{exact:true}).waitFor();
 await page.screenshot({path:`${out}/${theme}-sobre.png`});
 await page.getByRole('button',{name:'Abrir repositório',exact:true}).click();
 assert.deepEqual(await page.evaluate(()=>window.__calls.filter(x=>x[0]==='open_about_link').at(-1)),['open_about_link',{url:'https://github.com/s3xz/FastWisper'}]);
 await page.getByRole('button',{name:'Abrir perfil',exact:true}).click();
 assert.deepEqual(await page.evaluate(()=>window.__calls.filter(x=>x[0]==='open_about_link').at(-1)),['open_about_link',{url:'https://github.com/s3xz'}]);
 await menu.getByRole('button',{name:'Ajuda',exact:true}).click();
 await page.getByText(/fastwisper doctor/).waitFor();
 await page.screenshot({path:`${out}/${theme}-ajuda.png`});
 await page.getByRole('button',{name:'Abrir o guia',exact:true}).click();
 assert.equal(await page.evaluate(()=>window.__calls.at(-1)[0]),'open_guide');
 await page.getByRole('button',{name:'Conferir o sistema',exact:true}).click();
 await page.getByRole('dialog',{name:'Sistema',exact:true}).waitFor();
 await page.locator('#conferencia').waitFor();
 const content=page.locator('.content.settings-modal');
 await content.evaluate(el=>el.scrollTop=0);
 await page.getByRole('combobox',{name:'Processamento da transcrição',exact:true}).waitFor();
 assert.equal(await page.locator('.system-cards').count(),0);
 assert.equal(await page.locator('.settings-group').first().getAttribute('aria-label'),'Transcrição');
 assert.deepEqual(await page.getByRole('combobox',{name:'Processamento da transcrição',exact:true}).locator('option').allTextContents(),['Automático (melhor GPU)','Só o processador (CPU)','GPU Vulkan de exemplo']);
 await page.screenshot({path:`${out}/${theme}-sistema.png`});
 await page.evaluate(()=>window.__emit('navigate',{kind:'settings',tab:'vibe-coding'}));
 await page.getByRole('dialog',{name:'Vibe coding',exact:true}).waitFor();
 await page.getByRole('region',{name:'Terminais e harnesses de IA',exact:true}).waitFor();
 await page.waitForTimeout(2600);
 await page.screenshot({path:`${out}/${theme}-vibe-coding.png`});
 await page.getByRole('button',{name:'Editar lista',exact:true}).click();
 await page.getByText('Harnesses com janela própria',{exact:true}).waitFor();
 const harness=page.locator('.adjustment-row').filter({has:page.locator('#campo-inject-harness_apps-label')});
 await page.screenshot({path:`${out}/${theme}-harnesses.png`});
 await harness.getByRole('textbox',{name:'Adicionar item'}).fill('fixture.ai.desktop');
 await harness.getByRole('button',{name:'Adicionar',exact:true}).click();
 await page.waitForFunction(()=>window.__values['inject.harness_apps'].includes('fixture.ai.desktop'));
 await harness.getByRole('button',{name:'Remover fixture.ai.desktop',exact:true}).click();
 await page.waitForFunction(()=>window.__values['inject.harness_apps'].length===0);
 // O editor e a escrita são exercitados no harness de desenvolvimento; este fluxo confirma exposição.
 await page.getByRole('button',{name:'Voltar aos terminais',exact:true}).click();
}
await menu.getByRole('button',{name:'Sistema',exact:true}).click();
 await page.getByRole('combobox',{name:'Processamento da transcrição',exact:true}).selectOption('cpu');
 assert.equal(await page.evaluate(()=>window.__values['asr.gpu']),'cpu');
 await page.getByRole('combobox',{name:'Processamento da transcrição',exact:true}).selectOption('auto');
 assert.equal(await page.evaluate(()=>window.__values['asr.gpu']),'auto');
assert.equal(await page.evaluate(()=>window.__calls.filter(x=>x[0]==='vram_live').length),0);
assert.equal(await page.locator('vite-error-overlay').count(),0);
assert.deepEqual(errors,[]);
assert.deepEqual(network,[]);
await page.keyboard.press('Escape');
assert.equal(await page.getByRole('dialog').count(),0);
assert.equal(await page.getByRole('button',{name:'Configurações',exact:true}).evaluate(el=>el===document.activeElement),true);
// O evento de navegação profunda também abre Sobre.
await page.evaluate(()=>window.__emit('navigate',{kind:'settings',tab:'sobre'}));
await page.getByRole('dialog',{name:'Sobre',exact:true}).waitFor();
await page.setViewportSize({width:600,height:720});
assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=600),true);
await page.getByRole('button',{name:'Abrir perfil',exact:true}).waitFor();
await page.screenshot({path:`${out}/mobile-sobre.png`});
await context.addInitScript(()=>{ window.__initialDestination={kind:'settings',tab:'sobre'}; window.__failSettings=true; });
await page.reload();
await page.getByRole('dialog',{name:'Sobre',exact:true}).waitFor();
await page.getByRole('button',{name:'Abrir repositório',exact:true}).waitFor();
assert.equal(await page.getByRole('dialog').locator('.banner.err').count(),0);
await menu.getByRole('button',{name:'Ajuda',exact:true}).click();
await page.getByRole('button',{name:'Abrir o guia',exact:true}).waitFor();
assert.equal(await page.getByRole('dialog').locator('.banner.err').count(),0);
fs.writeFileSync(`${out}/checks.json`,JSON.stringify({url,title:await page.title(),browser:'Browser plugin not available; Playwright/Chromium headless',viewport:[1100,720],bridge:'Tauri falsa; nenhum comando real executado',checks:['menu Ajuda/Sobre','links exatos enviados ao comando','guia local e conferência','Sistema sem cartões nem polling exclusivo','rótulos GPU com valores preservados','grupo e lista de harnesses','navegação profunda via evento','Esc devolve foco','Sobre/Ajuda independem da config','Sobre em 600×720'],errors,network},null,2)+'\n');
await browser.close();
console.log('E0: interações e capturas verificadas.');
