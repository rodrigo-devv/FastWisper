# Pílula de gravação (`fastwisper.pill`)

Plugin `panel` do shell do Omarchy (Quickshell), instalado e habilitado por `packaging/omarchy/install.sh` em `~/.config/omarchy/plugins/fastwisper.pill/`.

## Estados

| Estado do daemon | Pílula |
|---|---|
| repouso, carregando, parado | oculta (nenhuma superfície mapeada) |
| gravando | 180×40, onda de 24 barras pelo nível do microfone |
| transcrevendo | shimmer nas barras |
| ditado falhou (`mic_silent`, microfone ausente…) | âmbar por 1,5 s e some, também em falhas repetidas |

Entrada e saída animadas (escala e opacidade, 150 ms). Fundo e barras usam o tema do Omarchy (`Color.background` a 92% e `Color.bar.text`); o âmbar vem dos tokens do hub.

| Gravando (sala silenciosa, daemon real) | Erro (daemon real, microfone ausente) |
|---|---|
| ![Gravando](docs/gravando.png) | ![Erro](docs/erro.png) |
| **Gravando com fala** (nível simulado) | **Processando** (estado simulado, dois quadros) |
| ![Gravando com fala](docs/gravando-fala-simulada.png) | ![Processando](docs/processando-simulado.png) |

## Como funciona

- **Estado:** `fastwisper status --follow --bar` (o mesmo da barra; só a `class` é usada).
- **Nível:** `fastwisper meter --follow`, um número por quadro (~25 Hz) só durante a gravação; nunca áudio. O valor novo entra no centro e corre para as bordas. Nível inválido (`NaN`) vale 0.
- **Limiar adaptativo:** cada gravação estima o piso de ruído do microfone como o percentil 10 dos últimos ~5 s de quadros (desce na hora numa pausa; só sobe se o ruído subir por segundos). A barra mostra só o que passa do piso por ~3,6 dB e enche ~15 dB acima disso. Microfone com ganho baixo e microfone ruidoso dão a mesma onda; silêncio e ruído constante ficam quase parados. Nenhum número depende de um microfone específico.
- Os dois rodam com `setpriv --pdeathsig TERM`: morrem com o shell. Eles mesmos reconectam quando o daemon cai; se o processo sair, a pílula o reinicia (o `meter` com espera crescente até 5 min, para um daemon antigo sem `meter` não virar loop; a espera volta a 5 s quando o processo fica de pé por 10 s ou lê um nível).
- **Âmbar de erro:** com daemon novo, a linha do `status --bar` traz `failures`, um contador de ditados que falharam; a pílula pisca só quando ele muda. Linha repetida ou mudada por outro motivo (VRAM, idioma), ASR que falha no início do daemon (`loading` → `error`) e cancel não piscam; duas falhas iguais seguidas piscam as duas. Daemon antigo (sem o campo): a heurística anterior, pela linha de texto.
- **Lógica pura:** onda, âmbar e espera do `meter` ficam em `PillLogic.js` (sem Qt), testado por `packaging/omarchy/tests/pill-logic.mjs` (node).
- **Foco e clique:** `WlrLayer.Overlay`, `keyboardFocus: None` e `mask: Region {}`. Zona exclusiva 0: fica acima da barra do Omarchy sem empurrar janelas.
- **Monitor:** o com foco no Hyprland (`Hyprland.focusedMonitor`) quando a pílula aparece; ela não troca de monitor no meio de uma gravação.
- **Privacidade:** nunca mostra texto ditado nem título de janela.

## Config

`[overlay]` é lido pela pílula, não pelo daemon: uma leitura de `fastwisper config list --json` a cada gravação ou transcrição que ela mostraria (e a cada erro), sem contador de respostas (uma resposta atrasada do press anterior não consome a do seguinte), então `fastwisper config set overlay.enabled false` vale na próxima gravação, sem reiniciar nada. Sem resposta em 400 ms (CLI antiga, por exemplo), vale o padrão: ligada, embaixo. `position` só aceita `"bottom"` por ora.

## Instalação

`install.sh` copia manifest, QML e `PillLogic.js` pelo ledger (rollback/uninstall removem), habilita com `omarchy plugin enable fastwisper.pill` se `.plugins` do `shell.json` ainda não a lista e só chama `omarchy-restart-shell` quando o QML/JS da pílula (ou o QML da barra) mudou. O opt-out é a config, não a remoção do plugin: uma reinstalação volta a habilitá-lo. Teste: `packaging/omarchy/tests/pill.sh`.
