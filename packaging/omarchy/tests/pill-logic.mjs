// Lógica pura da pílula (packaging/omarchy/pill-plugin/PillLogic.js), sem Qt:
// onda com limiar adaptativo ao ruído de cada microfone, NaN no nível,
// reconexão do `meter` e decisão do âmbar de erro. Chamado pelo pill.sh.
//   node packaging/omarchy/tests/pill-logic.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../pill-plugin/PillLogic.js', import.meta.url), 'utf8');
assert.ok(source.startsWith('.pragma library\n'), 'biblioteca QML sem estado');
const names = ['overlayConfig', 'newFloor', 'cleanLevel', 'waveHeight', 'nextMeterRetry', 'newErrorMemory', 'errorStep',
  'METER_RETRY_MS', 'METER_RETRY_MAX_MS', 'METER_HEALTHY_MS'];
const Logic = new Function(`${source.replace('.pragma library\n', '')}\nreturn { ${names.join(', ')} };`)();

// Ruído determinístico (LCG), sem Math.random.
function noise(seed) {
  let state = seed >>> 0;
  return () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    return state / 2 ** 32 - 0.5;
  };
}
// Quadros: { level, speech } a ~25 Hz.
function frames({ floor, jitter, count, seed = 1, speech = null }) {
  const random = noise(seed);
  const out = [];
  for (let i = 0; i < count; i += 1) {
    // Fala com sílabas: 6 quadros de voz, 2 de pausa (volta ao ruído).
    const voiced = speech !== null && i % 8 < 6;
    const level = (voiced ? speech : floor) + random() * 2 * jitter;
    out.push({ level, speech: voiced });
  }
  return out;
}
function run(sequence) {
  const floor = Logic.newFloor();
  return sequence.map((frame) => ({ ...frame, height: Logic.waveHeight(floor, frame.level) }));
}
const max = (list) => Math.max(...list.map((frame) => frame.height));
const mean = (list) => list.reduce((sum, frame) => sum + frame.height, 0) / list.length;
// A regra antiga, calibrada num microfone só, para mostrar o que muda.
const fixed = (level) => Math.max(0, Math.min(1, (level - 0.2) / 0.55));

// Nível inválido (NaN, texto, infinito, ausente) vale 0 e não entra no piso.
for (const bad of [NaN, 'abc', Infinity, -Infinity, null, undefined, '']) {
  assert.equal(Logic.cleanLevel(bad), null, String(bad));
  const floor = Logic.newFloor();
  assert.equal(Logic.waveHeight(floor, bad), 0);
  assert.equal(floor.history.length, 0);
}
assert.equal(Logic.cleanLevel('0.5'), 0.5);
assert.equal(Logic.cleanLevel(2), 1);
assert.equal(Logic.cleanLevel(-1), 0);
{
  // Um NaN no meio da fala não derruba o limiar nem acende a onda depois.
  const floor = Logic.newFloor();
  for (const frame of frames({ floor: 0.3, jitter: 0.02, count: 60 })) Logic.waveHeight(floor, frame.level);
  assert.equal(Logic.waveHeight(floor, NaN), 0);
  assert.ok(Logic.waveHeight(floor, 0.31) < 0.05);
}

// Ruído constante de sala, em três microfones bem diferentes: onda quase parada.
for (const [base, seed] of [[0.04, 2], [0.22, 3], [0.45, 4]]) {
  const quiet = run(frames({ floor: base, jitter: 0.02, count: 250, seed }));
  assert.ok(max(quiet) < 0.08, `ruído ${base}: máximo ${max(quiet)}`);
}
// A regra fixa deixava o microfone ruidoso mexendo sempre.
assert.ok(fixed(0.45) > 0.4);

// Fala depois do ruído: aparece forte nos três casos, inclusive com ganho
// baixo (fala só ~13 dB acima do ruído, tudo perto do fundo da escala).
for (const [base, speech, seed] of [[0.04, 0.26, 5], [0.22, 0.6, 6], [0.45, 0.78, 7]]) {
  const sequence = [
    ...frames({ floor: base, jitter: 0.015, count: 30, seed }),
    ...frames({ floor: base, jitter: 0.015, count: 200, seed: seed + 100, speech }),
  ];
  const result = run(sequence);
  const voiced = result.slice(30).filter((frame) => frame.speech);
  const pauses = result.slice(30).filter((frame) => !frame.speech);
  assert.ok(mean(voiced) > 0.55, `fala ${speech} sobre ${base}: média ${mean(voiced)}`);
  assert.ok(mean(pauses) < 0.1, `pausas sobre ${base}: média ${mean(pauses)}`);
}
// Com a regra fixa, a fala de ganho baixo mal aparecia.
assert.ok(fixed(0.26) < 0.15);

// Ruído que sobe no meio da gravação (ventoinha liga): em alguns segundos
// o piso acompanha e a onda volta a ficar quase parada.
{
  const result = run([
    ...frames({ floor: 0.1, jitter: 0.015, count: 60, seed: 8 }),
    ...frames({ floor: 0.3, jitter: 0.015, count: 240, seed: 9 }),
  ]);
  assert.ok(max(result.slice(60 + 130)) < 0.08, `depois de ~5 s: ${max(result.slice(190))}`);
}
// Ruído que desce: o piso desce na hora, a fala seguinte aparece.
{
  const result = run([
    ...frames({ floor: 0.35, jitter: 0.015, count: 60, seed: 10 }),
    ...frames({ floor: 0.1, jitter: 0.015, count: 20, seed: 11 }),
    ...frames({ floor: 0.1, jitter: 0.015, count: 40, seed: 12, speech: 0.4 }),
  ]);
  assert.ok(mean(result.slice(80).filter((frame) => frame.speech)) > 0.55);
}

// Reconexão do `meter`: dobra até 5 min; o início é 5 s.
assert.equal(Logic.METER_RETRY_MS, 5000);
assert.equal(Logic.nextMeterRetry(5000), 10000);
assert.equal(Logic.nextMeterRetry(200000), Logic.METER_RETRY_MAX_MS);
assert.equal(Logic.nextMeterRetry(Logic.METER_RETRY_MAX_MS), Logic.METER_RETRY_MAX_MS);
assert.ok(Logic.METER_HEALTHY_MS > 0);

// Config do overlay numa leitura só (`config list --json`).
const list = (enabled, position = 'bottom') => JSON.stringify([
  { key: 'audio.sounds', value: true }, { key: 'overlay.enabled', value: enabled }, { key: 'overlay.position', value: position },
]);
assert.deepEqual(Logic.overlayConfig(list(false)), { enabled: false, position: 'bottom' });
assert.deepEqual(Logic.overlayConfig(list(true, 'top')), { enabled: true, position: 'bottom' });
for (const bad of ['', 'error: unknown command', '{}', 'null', '[1, null]']) {
  assert.deepEqual(Logic.overlayConfig(bad), { enabled: true, position: 'bottom' }, bad);
}

// Âmbar de erro: aplica uma sequência de linhas e devolve onde piscou.
function flashes(lines) {
  let memory = Logic.newErrorMemory();
  return lines.map((data) => {
    const line = JSON.stringify(data);
    const step = Logic.errorStep(memory, data, line);
    memory = step.memory;
    return step.flash;
  });
}
const tip = (headline, extra = '') => `FastWisper\n${headline}${extra}`;
const offline = { class: 'offline', tooltip: tip('FastWisper parado') };
const loading = (failures) => ({ class: 'loading', tooltip: tip('Carregando o modelo…'), failures });
const idle = (failures) => ({ class: 'idle', tooltip: tip('Pronto'), failures });
const recording = (failures) => ({ class: 'recording', tooltip: tip('Gravando…'), failures });
const error = (failures, key = 'mic_silent', extra = '') =>
  ({ class: 'error', tooltip: tip(`Falhou: ${key}`, extra), failure_key: key, failures });

// Daemon novo: ASR que falha no início (loading → error) não pisca.
assert.deepEqual(flashes([offline, loading(0), error(0, 'asr_unavailable')]), [false, false, false]);
// Falha de um ditado pisca; a mesma linha de novo (outro motivo) ou com a
// dica mudada (VRAM) não; uma segunda falha idêntica pisca de novo.
assert.deepEqual(
  flashes([offline, idle(0), recording(0), error(1), error(1), error(1, 'mic_silent', '\nPerfil: jogo'), error(2)]),
  [false, false, false, true, false, false, true],
);
// Press recusado (sem passar por recording) também conta.
assert.deepEqual(flashes([offline, error(3), error(4, 'device_unavailable')]), [false, false, true]);
// Cancel volta à falha anterior sem piscar.
assert.deepEqual(flashes([offline, error(1), recording(1), error(1)]), [false, false, false, false]);
// Daemon reiniciado (contador volta a 0): a primeira linha é a base.
assert.deepEqual(flashes([offline, error(5), offline, error(0), error(1)]), [false, false, false, false, true]);
// Contador que não é número válido: comportamento antigo.
assert.deepEqual(flashes([offline, idle('x'), error('x')]), [false, false, true]);

// Daemon antigo (sem `failures`): a heurística anterior, intacta.
const old = (data) => { const copy = { ...data }; delete copy.failures; return copy; };
assert.deepEqual(flashes([offline, old(error(0))]), [false, false]);
assert.deepEqual(flashes([offline, old(idle(0)), old(error(0)), old(error(0))]), [false, false, true, true]);
assert.deepEqual(flashes([offline, old(error(0)), old(recording(0)), old(error(0))]), [false, false, false, false]);
assert.deepEqual(flashes([offline, old(loading(0)), old(error(0, 'asr_unavailable'))]), [false, false, true]);

console.log('lógica da pílula: PASS (limiar adaptativo em 3 microfones, NaN, ruído que sobe/desce, meter, âmbar por contador e legado)');
