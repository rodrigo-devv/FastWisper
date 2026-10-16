.pragma library
// Lógica pura da pílula do FastWisper, sem Qt: a altura da onda adaptada ao
// ruído de cada microfone e a decisão do âmbar de erro. Pill.qml importa este
// arquivo; packaging/omarchy/tests/pill-logic.mjs (node) o testa.

// --- Onda (limiar adaptativo) ---
// `level` do `meter --follow`: 0..1 (dBFS −60..0, linear). Cada gravação
// estima o piso de ruído do microfone como o percentil 10 dos últimos
// FLOOR_WINDOW quadros (~5 s a 25 Hz): numa pausa o piso desce na hora; ele
// só sobe se o ruído subir de verdade por segundos (fala tem pausas entre as
// sílabas, que mantêm o percentil baixo). A barra mostra só o que passa do
// piso por MARGIN (~3,6 dB, acima da flutuação de um ruído constante) e chega
// ao máximo SPAN acima disso (~15 dB). Ganho baixo (fala e ruído mais baixos)
// e ruído alto (os dois mais altos) dão a mesma onda: nenhum número depende de
// um microfone específico.
var FLOOR_WINDOW = 120
var FLOOR_PERCENTILE = 0.1
var MARGIN = 0.06
var SPAN = 0.25

// Estado do piso; um novo a cada gravação.
function newFloor() {
  return { history: [] }
}

// Nível válido 0..1; NaN, infinito ou ausente viram null.
function cleanLevel(value) {
  if (value === null || value === undefined || value === "") return null
  var number = Number(value)
  if (!isFinite(number)) return null
  return Math.max(0, Math.min(1, number))
}

// Altura 0..1 da barra para um quadro, atualizando o piso. Quadro inválido
// vale 0 e não entra no piso (um NaN não derruba o limiar).
function waveHeight(floor, value) {
  var level = cleanLevel(value)
  if (level === null) return 0
  var history = floor.history
  history.push(level)
  if (history.length > FLOOR_WINDOW) history.shift()
  var sorted = history.slice().sort(function(a, b) { return a - b })
  var noise = sorted[Math.floor((sorted.length - 1) * FLOOR_PERCENTILE)]
  return Math.max(0, Math.min(1, (level - noise - MARGIN) / SPAN))
}

// --- Reconexão do `meter` ---
// Daemon antigo recusa o `meter` e o processo sai na hora: a espera dobra até
// 5 min. Processo que ficou de pé (conectou) ou leu um nível volta ao início.
var METER_RETRY_MS = 5000
var METER_RETRY_MAX_MS = 300000
var METER_HEALTHY_MS = 10000

function nextMeterRetry(current) {
  return Math.min(METER_RETRY_MAX_MS, Math.max(METER_RETRY_MS, current) * 2)
}

// --- Config ---
// `[overlay]` da saída de `fastwisper config list --json` (lista de
// {key, value}). Saída ilegível (CLI antiga) vale o padrão: ligada, embaixo.
// Só existe "bottom" por ora; posição desconhecida cai nela.
function overlayConfig(text) {
  var out = { enabled: true, position: "bottom" }
  var list
  try { list = JSON.parse(text) } catch (e) { return out }
  if (!Array.isArray(list)) return out
  for (var i = 0; i < list.length; i++) {
    var item = list[i]
    if (!item || typeof item !== "object") continue
    if (item.key === "overlay.enabled") out.enabled = item.value !== false
  }
  return out
}

// --- Âmbar de erro ---
// Memória entre as linhas do `status --follow --bar`.
function newErrorMemory() {
  return { state: "offline", line: "", lastFailure: "", failureBeforeRecording: "", failures: null }
}

// Decide se a linha nova acende o âmbar. Devolve { flash, memory }.
//
// Daemon novo (`failures` na linha, contador monotônico de ditados que
// falharam): pisca só quando o contador muda. A primeira linha depois de
// conectar é a base (o daemon recém-iniciado com o ASR falho, `loading` →
// `error`, não pisca); uma linha repetida ou mudada por outro motivo (VRAM,
// idioma) não pisca; duas falhas iguais seguidas piscam as duas vezes; cancel
// não conta.
//
// Daemon antigo (sem `failures`): a heurística anterior pela linha de texto.
function errorStep(memory, data, line) {
  var previous = memory.state
  var next = String(data.class || "idle")
  // Chave estável da falha: trocar o idioma não transforma cancel em erro.
  // Fallback para consumidores antigos sem a chave, só com a manchete.
  var failure = next === "error"
    ? String(data.failure_key || String(data.tooltip || "").split("\n")[1] || "error") : ""
  var counter = typeof data.failures === "number" && isFinite(data.failures) ? data.failures : null
  var failureBeforeRecording = memory.failureBeforeRecording
  if (next === "recording" && previous !== "recording") failureBeforeRecording = memory.lastFailure
  var flash
  if (counter !== null) {
    flash = next === "error" && memory.failures !== null && counter !== memory.failures
  } else {
    // O daemon só publica quando algo muda; uma linha de erro idêntica à
    // anterior é outra tentativa que falhou igual. Vindo de offline é o estado
    // do daemon recém-conectado. O cancel volta à falha de antes da gravação.
    var repeated = line === memory.line
    var cancelled = previous === "recording" && failure === failureBeforeRecording
    flash = next === "error" && !cancelled
      && (repeated || (previous !== "error" && previous !== "offline"))
  }
  return {
    flash: flash,
    memory: {
      state: next,
      line: line,
      lastFailure: failure,
      failureBeforeRecording: failureBeforeRecording,
      failures: counter
    }
  }
}
