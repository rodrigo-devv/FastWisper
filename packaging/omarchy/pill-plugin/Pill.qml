// Pílula do FastWisper (fase 2). Instalada por packaging/omarchy/install.sh.
// Lê `fastwisper status --follow --bar` (estado, como a barra) e
// `fastwisper meter --follow` (nível do microfone, um número por quadro;
// nunca áudio). Nunca mostra texto ditado nem título de janela: só o estado.
// Oculta em repouso; gravando, onda de 24 barras na linguagem do ícone da
// barra, com limiar adaptado ao ruído de cada microfone; transcrevendo,
// shimmer; ditado que falhou, âmbar por 1,5 s. A lógica pura (onda e âmbar)
// fica em PillLogic.js, testada por packaging/omarchy/tests/pill-logic.mjs.
// Camada Overlay sem
// teclado e com máscara de entrada vazia: não rouba foco nem clique.
// `[overlay] enabled/position` vem de `fastwisper config list --json`, relido
// a cada gravação (a config muda ao vivo pela CLI ou pelo hub).
import QtQuick
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import Quickshell.Hyprland
import qs.Commons
import "PillLogic.js" as Logic

Item {
  id: root

  readonly property string cli: "@FASTWISPER_CLI@"
  readonly property int barCount: 24
  readonly property int pillWidth: 180
  readonly property int pillHeight: 40
  readonly property int errorMs: 1500

  // `class` da linha do status: offline, loading, idle, recording, transcribing, error.
  property string fwState: "offline"
  // Memória do âmbar entre linhas do status (Logic.errorStep).
  property var errorMemory: Logic.newErrorMemory()
  // Piso de ruído da gravação em andamento (Logic.waveHeight).
  property var noiseFloor: Logic.newFloor()
  property bool errorFlash: false
  property bool overlayEnabled: true
  property string overlayPosition: "bottom"
  // Leitura da config em andamento (`config list --json`, uma por press).
  property bool configPending: false
  // Nível por barra (0..1). O valor novo entra no centro e corre para as bordas.
  property var levels: []
  property var pinnedScreen: null

  readonly property string mode: errorFlash ? "error"
    : fwState === "recording" ? "recording"
    : fwState === "transcribing" ? "processing" : ""
  readonly property bool shown: mode !== "" && overlayEnabled && !configPending

  // Tema do Omarchy; os tokens do hub só entram no âmbar do erro, que o
  // tema não define.
  readonly property color surface: Util.alpha(Color.background, 0.92)
  readonly property color ink: Color.bar.text
  readonly property color amber: "#f2c35b"

  function zeros() {
    var out = []
    for (var i = 0; i < root.barCount; i++) out.push(0)
    return out
  }

  Component.onCompleted: root.levels = root.zeros()

  function update(line) {
    var data
    try { data = JSON.parse(line) } catch (e) { return }
    var previous = root.fwState
    // Falha de um ditado (mic_silent, microfone ausente...): avisa e some.
    // Daemon novo: decide pelo contador `failures`; antigo, pela linha.
    var step = Logic.errorStep(root.errorMemory, data, line)
    var next = step.memory.state
    var flash = step.flash
    root.errorMemory = step.memory
    var wasShown = root.mode !== ""
    if (next === "recording" && previous !== "recording") root.noiseFloor = Logic.newFloor()
    // Ordem das atribuições: o modo nunca passa por "" entre dois estados
    // visíveis (a pílula não pisca).
    if (flash) {
      root.errorFlash = true
      errorTimer.restart()
    }
    root.fwState = next
    if (next === "recording") root.errorFlash = false
    // Press curtíssimo pode pular direto para transcribing: também fixa
    // monitor e config do momento.
    if ((next === "recording" || next === "transcribing" || flash) && !wasShown) root.begin()
    if (next !== "recording") root.levels = root.zeros()
  }

  // Monitor com foco e config do momento; fixa o monitor até a pílula sumir.
  function begin() {
    var focused = Hyprland.focusedMonitor
    var name = focused ? String(focused.name || "") : ""
    var screens = Quickshell.screens
    root.pinnedScreen = screens.length > 0 ? screens[0] : null
    for (var i = 0; i < screens.length; i++)
      if (screens[i].name === name) root.pinnedScreen = screens[i]
    // Uma leitura só, sem contador: uma resposta atrasada do press anterior
    // não consome a do seguinte (no máximo traz a config de instantes antes).
    root.configPending = true
    readConfig.running = true
    configTimeout.restart()
  }

  function pushLevel(line) {
    if (root.fwState !== "recording") return
    var data
    try { data = JSON.parse(line) } catch (e) { return }
    // Só o que passa do piso de ruído deste microfone; NaN vale 0.
    var v = Logic.waveHeight(root.noiseFloor, data.level)
    var half = root.barCount / 2
    var next = root.levels.slice()
    for (var i = 0; i < half - 1; i++) next[i] = next[i + 1]
    for (var j = root.barCount - 1; j > half; j--) next[j] = next[j - 1]
    next[half - 1] = v
    next[half] = v
    root.levels = next
  }

  // Envelope em sino: o centro sobe mais, como no ícone de cinco barras.
  function envelope(i) {
    var x = (i + 0.5) / root.barCount - 0.5
    return 0.35 + 0.65 * Math.cos(x * Math.PI)
  }

  Timer {
    id: errorTimer
    interval: root.errorMs
    onTriggered: root.errorFlash = false
  }

  // Config que não responde (CLI sem `config`, por exemplo) vale o padrão.
  Timer {
    id: configTimeout
    interval: 400
    onTriggered: root.configPending = false
  }

  Process {
    id: readConfig
    command: [root.cli, "config", "list", "--json"]
    stdout: StdioCollector {
      onStreamFinished: {
        var overlay = Logic.overlayConfig(text)
        root.overlayEnabled = overlay.enabled
        root.overlayPosition = overlay.position
        root.configPending = false
        configTimeout.stop()
      }
    }
  }

  // Filhos diretos do shell com pdeathsig: somem junto se o shell reiniciar
  // (os dois comandos só notam o leitor fechado quando escrevem). Eles já
  // reconectam sozinhos quando o daemon cai; os Timers cobrem a saída do
  // próprio processo (CLI trocada, crash).
  Process {
    id: follow
    command: ["setpriv", "--pdeathsig", "TERM", root.cli, "status", "--follow", "--bar", "--localized"]
    running: true
    stdout: SplitParser {
      onRead: function(data) { root.update(data) }
    }
    onExited: {
      root.update('{"class":"offline"}')
      followRestart.start()
    }
  }

  Process {
    id: meter
    command: ["setpriv", "--pdeathsig", "TERM", root.cli, "meter", "--follow"]
    running: true
    stdout: SplitParser {
      onRead: function(data) {
        meterRestart.interval = Logic.METER_RETRY_MS
        root.pushLevel(data)
      }
    }
    // Daemon antigo responde `unknown_command` e o `meter` sai na hora: a
    // espera dobra até 5 min em vez de respawnar em loop curto. Processo que
    // fica de pé conectou (ou espera o daemon): a espera volta ao início.
    onRunningChanged: {
      if (running) meterHealthy.restart()
      else meterHealthy.stop()
    }
    onExited: meterRestart.start()
  }

  Timer {
    id: meterHealthy
    interval: Logic.METER_HEALTHY_MS
    onTriggered: if (meter.running) meterRestart.interval = Logic.METER_RETRY_MS
  }

  Timer {
    id: followRestart
    interval: 3000
    onTriggered: follow.running = true
  }

  Timer {
    id: meterRestart
    interval: Logic.METER_RETRY_MS
    onTriggered: {
      meter.running = true
      interval = Logic.nextMeterRetry(interval)
    }
  }

  PanelWindow {
    id: panel
    screen: root.pinnedScreen
    // Mapeada só enquanto aparece ou some: em repouso, nenhuma superfície.
    visible: root.shown || card.opacity > 0
    // Embaixo, ao centro; a zona exclusiva 0 respeita a da barra do Omarchy,
    // então a pílula fica acima dela sem empurrar janelas.
    anchors.bottom: root.overlayPosition === "bottom"
    margins.bottom: 12
    implicitWidth: root.pillWidth + 16
    implicitHeight: root.pillHeight + 16
    color: "transparent"
    exclusionMode: ExclusionMode.Normal
    exclusiveZone: 0
    WlrLayershell.namespace: "fastwisper-pill"
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: WlrKeyboardFocus.None
    // Só visual: região de entrada vazia, cliques passam para baixo.
    mask: Region {}

    Rectangle {
      id: card
      anchors.centerIn: parent
      width: root.pillWidth
      height: root.pillHeight
      radius: height / 2
      color: root.surface
      border.width: 1
      border.color: root.mode === "error" ? root.amber : Util.alpha(root.ink, 0.14)
      opacity: root.shown ? 1 : 0
      scale: root.shown ? 1 : 0.85

      Behavior on opacity { NumberAnimation { duration: 150; easing.type: Easing.OutCubic } }
      Behavior on scale { NumberAnimation { duration: 150; easing.type: Easing.OutCubic } }
      Behavior on border.color { ColorAnimation { duration: 150 } }

      // Shimmer da transcrição: um brilho que percorre as barras.
      property real sweep: -0.3
      NumberAnimation on sweep {
        running: root.mode === "processing" && panel.visible
        from: -0.3
        to: 1.3
        duration: 1100
        loops: Animation.Infinite
      }

      Row {
        id: wave
        anchors.centerIn: parent
        spacing: 3
        readonly property int barWidth: 3
        readonly property real maxHeight: card.height - 16

        Repeater {
          model: root.barCount

          Rectangle {
            id: stick
            required property int index
            readonly property real envelope: root.envelope(index)
            readonly property real rest: 0.22 * envelope
            readonly property real amount: root.mode === "recording"
              ? Math.max(rest, envelope * root.levels[index]) : rest
            readonly property real distance: index / (root.barCount - 1) - card.sweep

            anchors.verticalCenter: parent.verticalCenter
            width: wave.barWidth
            height: Math.max(width, Math.round(wave.maxHeight * amount))
            radius: width / 2
            color: root.mode === "error" ? root.amber : root.ink
            opacity: root.mode === "processing"
              ? 0.3 + 0.7 * Math.exp(-(distance * distance) / 0.02) : 1

            Behavior on height { NumberAnimation { duration: 70 } }
            Behavior on color { ColorAnimation { duration: 150 } }
          }
        }
      }
    }
  }
}
