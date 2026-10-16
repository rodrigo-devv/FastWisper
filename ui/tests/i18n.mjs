// Catálogos e referências: roda no gate `npm run check`, sem navegador/rede.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
const root = new URL('../src/', import.meta.url);
const read = file => JSON.parse(fs.readFileSync(new URL(file, root), 'utf8'));
const en = read('lib/i18n/en.json'), pt = read('lib/i18n/pt-BR.json');
function validate(a, b) {
  assert.deepEqual(Object.keys(a).sort(), Object.keys(b).sort(), 'chaves faltando entre idiomas');
  for (const [key, message] of Object.entries(a)) {
    assert.ok(message.trim(), key);
    assert.deepEqual((message.match(/\{\d+\}/g) ?? []).sort(), (key.match(/\{\d+\}/g) ?? []).sort(), key);
  }
}
validate(en, pt);
const missing = { ...en }; delete missing['Idioma do FastWisper'];
assert.throws(() => validate(missing, pt), /chaves faltando/);
for (const field of read('lib/settings/fields.json')) {
  for (const object of [field, ...field.options ?? []]) for (const key of ['label', 'help', 'placeholder', 'unit']) {
    if (object[key]) assert.ok(object[key] in en, `${field.key}: ${key}`);
    if (key === 'label' && en[object[key]] === pt[object[key]]) {
      assert.ok(['English', 'Português (Brasil)', 'pt', 'en', 'Formal', 'Casual', 'Local', 'Snippets'].includes(object[key]), `${field.key}: rótulo sem tradução explícita ${object[key]}`);
    }
  }
}
function scan(directory) {
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) scan(file);
    else if (/\.(svelte|ts)$/.test(file)) {
      const source = fs.readFileSync(file, 'utf8');
      for (const match of source.matchAll(/\b(?:t|source)\(("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')/g)) {
        const text = match[1].startsWith('"') ? JSON.parse(match[1]) : match[1].slice(1, -1).replaceAll("\\'", "'");
        assert.ok(text in en, `${file}: chave ausente ${text}`);
      }
    }
  }
}
scan(fileURLToPath(root));
// Rótulos de códigos do daemon (`label(MAP, código)`) também são chaves.
const activity = fs.readFileSync(new URL('lib/activity.ts', root), 'utf8');
for (const [, body] of activity.matchAll(/export const [A-Z_]+: Record<[^>]+> = \{([^}]*)\}/g)) {
  for (const [, text] of body.matchAll(/:\s*["']([^"']+)["']/g)) {
    assert.ok(text in en || text === 'Snippet', `activity.ts: chave ausente ${text}`);
  }
}
console.log(`i18n: PASS (${Object.keys(en).length} chaves, paridade, parâmetros e referências)`);

// A lógica de apresentação QML é JS local: nenhum shell/Quickshell real.
const qml = fs.readFileSync(new URL('../../packaging/omarchy/pill-plugin/Pill.qml', import.meta.url), 'utf8');
const begin = qml.indexOf('  function update(line) {') + '  function update(line) {'.length;
const end = qml.indexOf('\n  // Monitor com foco', begin);
assert.ok(begin > 0 && end > begin);
const body = qml.slice(begin, end).trim().replace(/}$/, '');
// `import "PillLogic.js" as Logic`: a biblioteca pura, sem o `.pragma`.
const logicSource = fs.readFileSync(new URL('../../packaging/omarchy/pill-plugin/PillLogic.js', import.meta.url), 'utf8')
  .replace('.pragma library\n', '');
const Logic = new Function(`${logicSource}\nreturn { newFloor, newErrorMemory, errorStep, waveHeight };`)();
const pill = { fwState:'offline', errorMemory:Logic.newErrorMemory(), noiseFloor:Logic.newFloor(), errorFlash:false,
  levels:[], get mode(){return this.errorFlash ? 'error' : this.fwState === 'recording' ? 'recording' : '';},
  begin(){}, zeros(){return [];} };
let flashes = 0;
const update = new Function('root', 'errorTimer', 'line', 'Logic', body);
const event = data => update(pill, {restart(){flashes++;}}, JSON.stringify(data), Logic);
event({class:'error',failure_key:'mic_silent',tooltip:'FastWisper\nMicrofone mudo'});
event({class:'recording'});
event({class:'error',failure_key:'mic_silent',tooltip:'FastWisper\nSilent microphone'});
assert.equal(pill.errorFlash,false,'idioma novo não transforma cancelamento em falha');
assert.equal(flashes,0);
event({class:'recording'});
event({class:'error',failure_key:'device_lost',tooltip:'FastWisper\nMicrophone disconnected'});
assert.equal(pill.errorFlash,true,'falha nova continua avisando');
console.log('pill-i18n: PASS (chave de falha estável entre idiomas)');
