// QA de UI com ponte Tauri inteiramente falsa: nunca chama a CLI, o daemon ou o Ollama.
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ?? 'playwright');
import fs from 'node:fs';
import assert from 'node:assert/strict';
import os from 'node:os';
import path from 'node:path';
const out=process.env.EVIDENCE_DIR ?? '/tmp/fw-h1-evidence'; fs.mkdirSync(out,{recursive:true});
const fixture = {
  "ui.language": "pt-BR",
 'hub.theme':'auto','audio.device':'fixture.microphone','asr.languages':['pt','en'],'hotkey.max_recording_secs':300,
 'audio.sounds':true,'audio.sound_volume':.4,'overlay.enabled':true,'overlay.position':'bottom',
 'asr.model':'/fixture/ggml-small.bin','asr.gpu':'auto','llm.enabled':true,'llm.model':'qwen3:4b-instruct',
 'llm.style':'casual','llm.timeout_ms':2000,'llm.skip_below_words':3,'llm.format_in_terminals':false,
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
   if(cmd==='settings_get') { await new Promise(resolve=>setTimeout(resolve,50)); return {values:{...values},defaults:fixture,restart_keys:['asr.model','asr.gpu','asr.threads','hotkey.min_recording_ms','llm.endpoint','llm.engine','llm.remote_consent']}; }
   if(cmd==='settings_set') { if(window.__failNext) { window.__failNext=false; throw 'Falha simulada de gravação'; } for(const [key,value] of args.changes) values[key]=value; return {status:'applied',restart_required:args.changes.map(x=>x[0]).filter(x=>['asr.model','asr.gpu','asr.threads','hotkey.min_recording_ms','llm.endpoint','llm.engine','llm.remote_consent'].includes(x))}; }
   if(cmd==='ui_language') return 'pt-BR';
    if(cmd==='hub_theme') return values['hub.theme'];
   if(cmd==='theme_colors') return colors;
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
},{fixture,colors});
const page=await context.newPage(); const errors=[];
page.on('pageerror',e=>errors.push(e.message));page.on('console',m=>{if(['error','warning'].includes(m.type())) errors.push(m.text());});
await page.goto(process.env.HUB_URL || 'http://127.0.0.1:5193/');
await page.getByRole('link', {name:'Ver estatísticas: 1 semana, 123 palavras, 82 PPM', exact:true}).waitFor();
await page.evaluate(()=>{ window.__singleWord=true; window.__emit('history-cleared',null); });
await page.getByRole('link',{name:'Ver estatísticas: 1 semana, 1 palavra, 82 PPM',exact:true}).waitFor();
await page.evaluate(()=>{ window.__singleWord=false; window.__emit('history-cleared',null); });
await page.getByRole('link',{name:'Ver estatísticas: 1 semana, 123 palavras, 82 PPM',exact:true}).waitFor();
const modalOpener=page.getByRole('button',{name:'Configurações',exact:true});
await modalOpener.click();
const modal=page.getByRole('dialog',{name:'Geral',exact:true}); await modal.waitFor();
await page.locator('.modal').getByRole('radio',{name:'Automático',exact:true}).waitFor();
await page.evaluate(()=>document.fonts.ready);
assert.equal(await page.title(),'Início · FastWisper');
for(const theme of ['auto','dark','light']){
 await page.getByRole('button',{name:'Geral',exact:true}).click();
 await page.locator('.modal').getByRole('radio',{name:{auto:'Automático',dark:'Escuro',light:'Claro'}[theme],exact:true}).check();
 await page.waitForFunction(theme=>document.documentElement.dataset.theme===theme,theme);
 // Let only the mock toast expire for clean reference evidence.
 await page.waitForTimeout(2600);
 for(const [tab,label] of [['geral','Geral'],['sistema','Sistema'],['privacidade','Dados e privacidade']]){
  await page.getByRole('button',{name:label,exact:true}).click();
  await page.waitForTimeout(180);
  await page.screenshot({path:`${out}/${theme}-${tab}.png`});
  const content=page.locator('.content.settings-modal');
  const dimensions=await content.evaluate(el=>({scroll:el.scrollHeight,height:el.clientHeight}));
  for(let y=500,index=1;y<dimensions.scroll;y+=500,index++){
   await content.evaluate((el,y)=>el.scrollTop=y,y); await page.waitForTimeout(80);
   await page.screenshot({path:`${out}/${theme}-${tab}-scroll-${index}.png`});
  }
  // Also capture the complete content column beside the exported scroll reference.
  await content.evaluate(el => el.scrollTop=0);
  await page.evaluate(() => {
    document.querySelector('.modal').style.height='auto';
    document.querySelector('.veil').style.position='absolute';
    document.querySelector('.veil').style.height='auto';
    document.querySelector('.veil').style.minHeight='100vh';
    document.querySelector('.veil').style.alignItems='start';
    document.querySelector('.content.settings-modal').style.height='auto';
    document.querySelector('.content.settings-modal').style.overflow='visible';
  });
  await content.screenshot({path:`${out}/${theme}-${tab}-full.png`});
  await page.evaluate(() => {
    for(const selector of ['.modal','.veil','.content.settings-modal']) document.querySelector(selector).removeAttribute('style');
  });
 }
}
// Destructive action only invokes the mock after confirmation; Escape cancels just the inner dialog.
await page.getByRole('button',{name:'Dados e privacidade',exact:true}).click();
await page.getByRole('button',{name:'Apagar',exact:true}).click();
assert.equal(await page.evaluate(()=>window.__calls.filter(x=>x[0]==='history_clear').length),0);
const confirmationBox=await page.locator('dialog[open]').boundingBox();
assert(Math.abs(confirmationBox.x + confirmationBox.width/2 - 550)<2);
assert(Math.abs(confirmationBox.y + confirmationBox.height/2 - 360)<2);
await page.keyboard.press('Escape'); await page.getByRole('heading',{name:'Dados e privacidade',exact:true}).waitFor();
assert.equal(await page.locator('dialog[open]').count(),0);
await page.getByRole('button',{name:'Apagar',exact:true}).click();
await page.locator('dialog[open]').getByRole('button',{name:'Apagar',exact:true}).click();
await page.getByText('Histórico de uso apagado.',{exact:true}).waitFor();
await page.waitForFunction(()=>document.querySelector('.home-stats')?.textContent.includes('0 palavras'));
assert.equal(await page.evaluate(()=>window.__calls.filter(x=>x[0]==='history_clear').length),1);
await page.getByRole('switch',{name:'Modo 100% offline',exact:true}).click();
assert.deepEqual(await page.evaluate(()=>window.__calls.filter(x=>x[0]==='ollama_cloud_set').at(-1)),['ollama_cloud_set',{online:true}]);
assert.equal(await page.getByRole('switch',{name:'Modo 100% offline'}).getAttribute('aria-checked'),'false');
await page.getByRole('switch',{name:'Modo 100% offline',exact:true}).click();
assert.deepEqual(await page.evaluate(()=>window.__calls.filter(x=>x[0]==='ollama_cloud_set').at(-1)),['ollama_cloud_set',{online:false}]);
assert.equal(await page.getByRole('switch',{name:'Modo 100% offline'}).getAttribute('aria-checked'),'true');
await page.getByRole('button',{name:'Configurar',exact:true}).click();
await page.locator('#remoto-endereco').fill('https://api.example.com');
await page.locator('#remoto-modelo').fill('fixture-model');
assert.equal(await page.getByRole('button',{name:'Usar este provider'}).isDisabled(),true);
await page.locator('.consent input').check();
await page.getByRole('button',{name:'Usar este provider'}).click();
assert.equal(await page.getByRole('switch',{name:'Modo 100% offline'}).isDisabled(),true);
assert.equal(await page.getByRole('switch',{name:'Modo 100% offline'}).getAttribute('aria-checked'),'false');
await page.getByLabel('Nuvem online').waitFor();
await page.getByRole('button',{name:'Voltar ao Ollama local'}).click();
assert.equal(await page.getByRole('switch',{name:'Modo 100% offline'}).isEnabled(),true);
await page.getByRole('button',{name:'Geral',exact:true}).click();
await page.locator('#campo-audio-device').selectOption('fixture.usb');
await page.getByRole('button',{name:'Sistema',exact:true}).click();
await page.locator('#campo-asr-gpu').selectOption('cpu');
await page.getByText('reinício pendente',{exact:true}).waitFor();
await page.locator('.banner').getByRole('button',{name:'Reiniciar o FastWisper',exact:true}).click();
assert.equal(await page.getByText('reinício pendente',{exact:true}).count(),0);
await page.getByRole('button',{name:'Geral',exact:true}).click();
// Number inputs, failed select/theme writes, and language minimum retain consistent state.
await page.locator('#campo-hotkey-max_recording_secs').fill('120');
await page.locator('#campo-hotkey-max_recording_secs').dispatchEvent('change');
await page.waitForFunction(()=>window.__values['hotkey.max_recording_secs']===120);
await page.locator('#campo-hotkey-max_recording_secs').fill('0');
await page.locator('#campo-hotkey-max_recording_secs').dispatchEvent('change');
await page.getByText('Use de 1 a 300.',{exact:true}).waitFor();
assert.equal(await page.evaluate(()=>window.__values['hotkey.max_recording_secs']),120);
await page.locator('.modal').getByRole('checkbox',{name:'pt',exact:true}).click();
await page.locator('.modal').getByRole('checkbox',{name:'en',exact:true}).click();
await page.getByText('Escolha ao menos um.',{exact:true}).waitFor();
assert.equal(await page.locator('.modal').getByRole('checkbox',{name:'en',exact:true}).isChecked(),true);
await page.evaluate(()=>window.__failNext=true);
await page.locator('.modal').getByRole('radio',{name:'Escuro',exact:true}).click();
await page.getByText(/Não foi salvo: Falha simulada/).waitFor();
assert.equal(await page.locator('.modal').getByRole('radio',{name:'Claro',exact:true}).isChecked(),true);
await page.getByRole('button',{name:'Sistema',exact:true}).click();
const priorStyle=await page.locator('#campo-llm-style').inputValue();
await page.evaluate(()=>window.__failNext=true);
await page.locator('#campo-llm-style').selectOption('formal');
await page.waitForFunction(style=>document.querySelector('#campo-llm-style').value===style,priorStyle);
// A-10: modo de VRAM que não salvou volta ao modo real (o leitor de tela anuncia o certo).
await page.evaluate(()=>window.__failNext=true);
await page.locator('#campo-performance-vram').selectOption('jogo');
await page.waitForFunction(()=>document.querySelector('#campo-performance-vram').value==='automatico');
assert.equal(await page.evaluate(()=>window.__values['performance.vram']),'automatico');
await page.getByRole('combobox',{name:'Modelo de transcrição',exact:true}).selectOption('/fixture/ggml-medium.bin');
assert.equal(await page.evaluate(()=>window.__values['asr.model']),'/fixture/ggml-small.bin');
await page.getByRole('button',{name:'Baixar e usar'}).click();
await page.getByRole('button',{name:'Baixando 40%…',exact:true}).waitFor();
await page.waitForFunction(()=>window.__values['asr.model']==='/fixture/ggml-medium.bin');
await page.getByRole('button',{name:'Geral',exact:true}).click();
// Trap and restore focus, including close button and veil.
await page.locator('.modal-menu button').first().focus(); await page.keyboard.press('Shift+Tab');
assert.equal(await page.evaluate(()=>document.querySelector('.modal').contains(document.activeElement)),true);
await page.keyboard.press('Escape'); assert.equal(await page.locator('.modal').count(),0);
assert.equal(await page.evaluate(()=>document.activeElement.textContent),'Configurações');
await page.getByRole('button',{name:'Configurações',exact:true}).click(); await page.getByRole('button',{name:'Fechar',exact:true}).click();
await page.getByRole('button',{name:'Configurações',exact:true}).click(); await page.locator('.veil').click({position:{x:5,y:5}});
assert.equal(await page.locator('.modal').count(),0);
await page.setViewportSize({width:600,height:720}); await page.getByRole('button',{name:'Configurações',exact:true}).click();
await page.screenshot({path:`${out}/mobile-geral.png`});
assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=600),true);
const interactionCalls=await page.evaluate(()=>window.__calls);
await page.setViewportSize({width:1100,height:720});
await page.reload();
await page.getByRole('button',{name:'Configurações',exact:true}).click();
await page.getByRole('button',{name:'Ajuda',exact:true}).click();
await page.getByRole('button',{name:'Conferir o sistema',exact:true}).click();
await page.getByRole('button',{name:'Conferir de novo',exact:true}).waitFor();
await page.waitForFunction(()=>document.querySelector('.content.settings-modal').scrollTop>500);
await page.getByRole('button',{name:'Fechar',exact:true}).click();
await page.goto(new URL('/historico', page.url()).href);
await page.getByRole('heading',{name:'Histórico',exact:true}).waitFor();
await page.getByText('Ditados',{exact:true}).waitFor();
await page.screenshot({path:`${out}/historico.png`});
await page.getByRole('button',{name:'Configurações',exact:true}).click();
await page.getByRole('button',{name:'Dados e privacidade',exact:true}).click();
await page.getByRole('button',{name:'Apagar',exact:true}).click();
await page.locator('dialog[open]').getByRole('button',{name:'Apagar',exact:true}).click();
await page.getByRole('button',{name:'Fechar',exact:true}).click();
await page.getByRole('heading',{name:'Nenhum ditado nos últimos 7 dias',exact:true}).waitFor();
await page.getByRole('radio',{name:'30 dias',exact:true}).click();
await page.getByRole('heading',{name:'Nenhum ditado nos últimos 30 dias',exact:true}).waitFor();
// Personalização permanece montada atrás do modal: só um feedback visível.
await page.goto(new URL('/personalizacao/estilo', page.url()).href);
await page.getByRole('heading',{name:'Estilo',exact:true}).waitFor();
await page.getByRole('button',{name:'Configurações',exact:true}).click();
await page.getByRole('button',{name:'Geral',exact:true}).click();
await page.locator('.modal').getByRole('radio',{name:'Claro',exact:true}).check();
await page.getByText('Salvo e aplicado.',{exact:true}).waitFor();
assert.equal(await page.locator('.toast').count(),1);
assert.deepEqual(errors,[]);
fs.writeFileSync(`${out}/checks.json`,JSON.stringify({url:page.url(),title:await page.title(),viewport:[1100,720],colors,errors,checks:['singular stats and accessible values','history page refreshed on history-cleared','destructive dialog centered','single feedback over customization','theme live save','three tabs and scrolls in three themes','confirmation cancel and confirm','home stats refreshed on history-cleared','offline inversion','remote consent and disabled offline','restart banner','Esc X veil close','focus containment and restore','mobile overflow','help deep link after config loads','integer validation','last language retained','failed select and theme restores saved choice','model download requires action and renders progress'],calls:[...interactionCalls,...await page.evaluate(()=>window.__calls)]},null,2));
await browser.close(); console.log('QA passed: '+out);
