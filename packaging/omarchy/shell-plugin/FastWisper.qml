// FastWisper na barra do Omarchy. Instalado por packaging/omarchy/install.sh.
// Lê `fastwisper status --follow --bar` (um JSON por mudança de estado, sem
// texto ditado). Ícone de onda sonora: na cor da barra em repouso, com o
// máximo contraste (branco no tema escuro) e as barras oscilando enquanto
// grava, cor de urgência com erro, apagado com o daemon parado ou
// carregando. Clique liga/desliga o hands-free; clique direito abre o hub
// (sem o hub instalado, a notificação diz o motivo que a CLI explica).
import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui

BarWidget {
  id: root
  moduleName: "fastwisper.dictation"

  readonly property string cli: "@FASTWISPER_CLI@"
  property string fwState: "offline"
  property string tip: "FastWisper"

  readonly property bool busy: fwState === "recording" || fwState === "transcribing"
  readonly property color foreground: bar ? bar.barForeground : Color.foreground
  readonly property color iconColor: fwState === "error"
    ? (bar ? bar.urgent : Color.urgent)
    : busy ? (foreground.hslLightness >= 0.5 ? "#ffffff" : "#000000") : foreground

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  function update(line) {
    var data
    try { data = JSON.parse(line) } catch (e) { return }
    root.fwState = String(data.class || "idle")
    root.tip = String(data.tooltip || "FastWisper")
  }

  // Filho direto do shell com pdeathsig: some junto se o shell reiniciar.
  Process {
    id: follow
    command: ["setpriv", "--pdeathsig", "TERM", root.cli, "status", "--follow", "--bar", "--localized"]
    running: true
    stdout: SplitParser {
      onRead: function(data) { root.update(data) }
    }
    onExited: restart.start()
  }

  // argv direto, sem shell: o caminho da CLI pode ter espaços ou aspas.
  Process {
    id: toggle
    command: [root.cli, "ctl", "toggle"]
  }

  // `fastwisper hub` sem o hub instalado sai com erro e explica o motivo no
  // stderr (no idioma da config): vira notificação em vez de silêncio. A saída
  // e o fim do stderr chegam em qualquer ordem; notifica quando os dois chegam.
  property int hubExit: -1
  property string hubMessage: ""
  property bool hubMessageDone: false

  function openHubNow() {
    if (openHub.running) return
    root.hubExit = -1
    root.hubMessage = ""
    root.hubMessageDone = false
    openHub.running = true
  }

  function hubFinished() {
    if (root.hubExit < 0 || !root.hubMessageDone) return
    if (root.hubExit !== 0) {
      var message = root.hubMessage.trim().replace(/^fastwisper: /, "")
      Quickshell.execDetached(["notify-send", "-a", "FastWisper", "FastWisper", message || "fastwisper hub"])
    }
    root.hubExit = -1
  }

  Process {
    id: openHub
    command: [root.cli, "hub"]
    stderr: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        root.hubMessage = text
        root.hubMessageDone = true
        root.hubFinished()
      }
    }
    onExited: function(exitCode) {
      root.hubExit = exitCode
      root.hubFinished()
    }
  }

  Timer {
    id: restart
    interval: 3000
    onTriggered: follow.running = true
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    slotSize: Style.bar.statusSlot
    tooltipText: root.tip
    dimmed: root.fwState === "offline" || root.fwState === "loading"
    iconComponent: Component {
      Item {
        id: wave
        // Proporções das cinco barras, da esquerda para a direita.
        readonly property var heights: [0.4, 0.75, 1.0, 0.62, 0.4]
        readonly property real barWidth: Math.max(2, Math.round(width / 8))

        Row {
          anchors.centerIn: parent
          spacing: wave.barWidth

          Repeater {
            model: wave.heights.length

            Rectangle {
              id: stick
              required property int index
              property real level: 1

              anchors.verticalCenter: parent.verticalCenter
              width: wave.barWidth
              height: Math.max(width, Math.round(wave.height * 0.85 * wave.heights[index] * level))
              radius: width / 2
              color: root.iconColor

              Behavior on color { ColorAnimation { duration: 140 } }

              // Gravando: cada barra oscila num ritmo próprio.
              SequentialAnimation on level {
                running: root.fwState === "recording"
                loops: Animation.Infinite
                NumberAnimation { to: 0.45; duration: 260 + stick.index * 70; easing.type: Easing.InOutSine }
                NumberAnimation { to: 1.0; duration: 260 + stick.index * 70; easing.type: Easing.InOutSine }
                onStopped: stick.level = 1
              }
            }
          }
        }
      }
    }
    onPressed: function(mouseButton) {
      if (mouseButton === Qt.RightButton) root.openHubNow()
      else if (mouseButton === Qt.LeftButton) toggle.running = true
    }
  }
}
