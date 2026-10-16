// Bridge Tauri falsa: nenhuma ação chega ao daemon, clipboard ou configuração reais.
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ?? 'playwright');
import os from 'node:os';
import path from 'node:path';
import fs from 'node:fs';
import assert from 'node:assert/strict';
const out=process.env.EVIDENCE_DIR ?? '/tmp/h3b-evidence'; fs.mkdirSync(out,{recursive:true});
const base=process.env.HUB_URL ?? 'http://127.0.0.1:5197';
// Automático com paleta sintética: nada do tema desta máquina entra em QA ou capturas (A-04).
const colors={background:'#15171c',foreground:'#e8e6df',accent:'#5fb3a1',surface:'#1e2128'};
const browser=await chromium.launch({headless:true, executablePath:process.env.BROWSER_BINARY || undefined});
const errors=[],network=[],results=[];
for(const theme of ['auto','dark','light']) {
 const context=await browser.newContext({viewport:{width:1100,height:720},reducedMotion:'reduce'});
 await context.addInitScript(({theme,colors})=>{
  window.isTauri=true;
  const callbacks=new Map(),listeners=new Map(); let serial=0;
  const vram={mode:'automatico',profile:'sob_demanda',game:false,asr:'unloaded',llm:'unloaded',vram_mb:0,unload_pending:false};
  const latency={count:0,p50:0,p95:0,max:0};
  const samples=['Revisar o roteiro da apresentação de sexta e separar os exemplos de ditado em português.','Ideia para o FastWisper: dar mais destaque ao estado do microfone sem interromper quem está escrevendo.','Remember to check the interface on a smaller screen before the next release.','Preparar o almoço e depois responder às mensagens da equipe.','Organizar os links da pesquisa em uma nota curta para consultar amanhã.','Começar pelo caminho mais simples e validar cada etapa com um exemplo real.'];
  window.qa={calls:[],view:{available:true,state:'idle',hands_free:false,note_session:false,asr:'ready',vram,last:null},values:{'experimental.notes':true,'hub.theme':theme,'llm.ollama_cloud':false,'llm.enabled':false,'llm.engine':'ollama','inject.clipboard_history':false},notes:samples.map((text,i)=>({id:i+1,ts:1728814200-i*3600,text,language:i===2?'en':'pt',words:text.split(' ').length}))};
  window.qa.emit=(event,payload)=>{for(const id of listeners.get(event)??[]) callbacks.get(id)?.({payload});};
  const emitStatus=()=>window.qa.emit('daemon-status',{...window.qa.view});
  window.__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener(){}};
  window.__TAURI_INTERNALS__={transformCallback(cb){const id=++serial;callbacks.set(id,cb);return id;},async invoke(cmd,args={}){
   window.qa.calls.push({cmd,args});
   if(cmd==='plugin:event|listen'){const list=listeners.get(args.event)??[];list.push(args.handler);listeners.set(args.event,list);return args.handler;}
   if(cmd==='plugin:event|unlisten')return;
   if(cmd==='ui_language') return 'pt-BR';
    if(cmd==='hub_theme')return window.qa.values['hub.theme'];
   if(cmd==='theme_colors')return colors;
   if(cmd==='hub_version')return '0.1.0';
   if(cmd==='initial_destination')return null;
   if(cmd==='daemon_view')return window.qa.view;
   if(cmd==='vram_total')return 16384;
   if(cmd==='settings_get')return {values:{...window.qa.values},defaults:{'experimental.notes':false},restart_keys:[]};
   if(cmd==='settings_set'){for(const [key,value] of args.changes)window.qa.values[key]=value;return {status:'applied',restart_required:[]};}
   if(cmd==='history_stats')return {days:90,empty:false,total:300,consecutive_weeks:3,words:12480,ppm:142,outcomes:[],deliveries:[],profiles:[],apps:[],asr_ms:latency,llm_ms:latency,e2e_ms:latency,load_ms:latency,llm_load_ms:latency};
   if(cmd==='notes_list')return window.qa.notes;
   if(cmd==='note_start'){if(!window.qa.values['experimental.notes'])throw Error('disabled');window.qa.view={...window.qa.view,state:'recording',note_session:true};emitStatus();return;}
   if(cmd==='note_cancel'){window.qa.view={...window.qa.view,state:'idle',note_session:false};emitStatus();return;}
   if(cmd==='note_save')throw Error('QA só cancela gravação');
   if(cmd==='notes_copy')return;
   if(cmd==='notes_delete'){window.qa.notes=window.qa.notes.filter(n=>n.id!==args.id);window.qa.emit('notes-changed',null);return;}
   if(cmd==='notes_clear'){window.qa.notes=[];window.qa.emit('notes-changed',null);return;}
   if(cmd==='ollama_cloud')return {cloud:false,disabled:true,error:null};
   if(cmd==='meter_start'){window.qa.meter=setInterval(()=>window.qa.emit('meter',{recording:true,level:.65}),40);return;}
   if(cmd==='meter_stop'){clearInterval(window.qa.meter);return;}
   throw Error('Unhandled mock '+cmd);
  }};
 },{theme,colors});
 const page=await context.newPage();page.on('pageerror',e=>errors.push(e.message));page.on('console',m=>{if(m.type()==='error')errors.push(m.text());});
 page.on('request',r=>{if(!r.url().startsWith(base)&&!r.url().startsWith('data:'))network.push(r.url());});
 await page.goto(base);await page.getByText('Revisar o roteiro', {exact:false}).waitFor();await page.evaluate(()=>document.fonts.ready);
 assert.equal(await page.title(),'Início · FastWisper');
 await page.screenshot({path:`${out}/${theme}-inicio-notas.png`});
 await page.getByRole('button',{name:'Gravar nota',exact:true}).click();await page.getByText('Gravando nota…',{exact:true}).waitFor();await page.waitForTimeout(1000);
 assert.equal(await page.locator('.wave span').count(),24);await page.screenshot({path:`${out}/${theme}-inicio-gravando.png`});
 await page.keyboard.press('Escape');await page.getByRole('button',{name:'Gravar nota',exact:true}).waitFor();assert.equal(await page.evaluate(()=>window.qa.notes.length),6);
 const reads=await page.evaluate(()=>window.qa.calls.filter(c=>c.cmd==='notes_list').length);
 await page.getByRole('button',{name:'Atualizar notas',exact:true}).click();
 await page.waitForFunction(reads=>window.qa.calls.filter(c=>c.cmd==='notes_list').length>reads,reads);
 await page.getByRole('button',{name:'Buscar notas',exact:true}).click();await page.getByRole('searchbox').fill('Remember');assert.equal(await page.locator('.recent li').count(),1);
 await page.getByRole('button',{name:'Copiar nota',exact:true}).click();assert((await page.evaluate(()=>window.qa.calls)).some(c=>c.cmd==='notes_copy'&&c.args.id===3));
 await page.getByRole('button',{name:'Visualização compacta',exact:true}).click();assert.equal(await page.locator('.recent li.compact').count(),1);
 await page.getByRole('button',{name:'Apagar nota',exact:true}).click();await page.getByText('Nenhuma nota encontrada').waitFor();assert.equal(await page.evaluate(()=>window.qa.notes.length),5);
 await page.getByRole('button',{name:'Configurações',exact:true}).click();await page.getByRole('button',{name:'Experimental',exact:true}).click();await page.screenshot({path:`${out}/${theme}-experimental.png`});await page.getByRole('switch',{name:'Notas por voz',exact:true}).click();
 await page.getByRole('button',{name:'Fechar',exact:true}).click();await page.getByRole('button',{name:'Gravar nota (Notas desligado)',exact:true}).waitFor();assert.equal(await page.locator('.recent li').count(),0);
 await page.screenshot({path:`${out}/${theme}-inicio-desligado.png`});
 await page.getByRole('button',{name:'Configurações',exact:true}).click();await page.getByRole('button',{name:'Dados e privacidade',exact:true}).click();assert.equal(await page.getByText('Apagar todas as notas',{exact:true}).count(),0);
 await page.getByRole('button',{name:'Experimental',exact:true}).click();await page.getByRole('switch',{name:'Notas por voz',exact:true}).click();
 await page.getByRole('button',{name:'Dados e privacidade',exact:true}).click();await page.getByRole('button',{name:'Apagar notas',exact:true}).click();await page.getByRole('dialog',{name:'Apagar todas as notas?',exact:true}).waitFor();
 await page.screenshot({path:`${out}/${theme}-apagar-confirmacao.png`});
 await page.getByRole('dialog',{name:'Apagar todas as notas?',exact:true}).getByRole('button',{name:'Apagar',exact:true}).click();
 assert.equal(await page.evaluate(()=>window.qa.notes.length),0);
 results.push({theme,viewport:'1100x720',recording:'cancelada sem salvar',waveBars:24,search:true,compact:true,copy:true,delete:true,disabled:true,clearConfirmation:true});
 await context.close();
}
assert.deepEqual(errors,[]);assert.deepEqual(network,[]);
fs.writeFileSync(`${out}/qa.json`,JSON.stringify({browser:'Browser plugin not available; Playwright',url:base,errors,network,results},null,2));
await browser.close();console.log('QA passou nos três temas');
