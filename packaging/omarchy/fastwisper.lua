-- FastWisper: atalhos de ditado para Omarchy/Hyprland (Lua, Hyprland 0.56+).
-- Instalado por packaging/omarchy/install.sh; não editar a cópia instalada.
--
-- Os atalhos vêm da config ([hotkey]; padrões abaixo):
--   ditar (segurar)   Alt+Space: grava; soltar a tecla ou qualquer modificador
--                     do atalho (esquerdo ou direito), em qualquer ordem,
--                     transcreve e cola
--   mãos livres       Alt+Shift+Space: liga/desliga a gravação
--   colar de novo     Super+Alt+V: cola o último ditado na janela com foco
--   Esc               cancela a gravação em andamento (fixo; o Esc segue para o app)
--
-- `fastwisper config set hotkey.dictate SUPER+D` (ou o hub) grava o arquivo
-- de atalhos e recarrega o Hyprland. O arquivo é só dados: cada linha passa
-- por padrões estritos e nada dele é executado. Faltando ou inválido, valem
-- os padrões abaixo.
--
-- Nada é ligado a uma tecla sozinha: um bind de release no Space faz o
-- Hyprland engolir o espaço digitado. A soltura é vista por um observador de
-- teclas que só existe enquanto o atalho de ditar está segurado; ele lê
-- apenas o código da tecla e não consome eventos. Os binds de ditado do
-- Omarchy (F9 e Super+Ctrl+X → Voxtype) não são tocados.

local cli = "@FASTWISPER_CLI@"
local hotkeys_file = "@FASTWISPER_HOTKEYS@"

-- Padrões, no formato do arquivo gerado (o teste lua_defaults_match_the_generator
-- confere com o gerador do fw-core). Keycodes xkb (evdev + 8), como o evento
-- `input.keyboard.key` os entrega: tecla principal e os dois lados de cada modificador.
local DEFAULTS = [[
version 1
dictate ALT+SPACE 65 64 108
hands_free ALT+SHIFT+SPACE
paste_last SUPER+ALT+V
]]
local KEY_ESC = 9
local MAX_BYTES = 4096
local PRESSED = 1
local RELEASED = 0

local MODIFIERS = { SUPER = true, CTRL = true, ALT = true, SHIFT = true }
local ACTIONS = { dictate = true, hands_free = true, paste_last = true }

-- "SUPER+ALT+V" → "SUPER + ALT + V"; nil se não for modificadores + uma tecla,
-- com ao menos um modificador além de Shift.
local function parse_combo(text)
  local parts = {}
  for part in (text .. "+"):gmatch("([^+]*)%+") do
    parts[#parts + 1] = part
  end
  if #parts < 2 or #parts > 5 then
    return nil
  end
  local strong, seen = false, {}
  for i, part in ipairs(parts) do
    if not part:match("^[%u%d_]+$") or seen[part] then
      return nil
    end
    seen[part] = true
    if i < #parts then
      if not MODIFIERS[part] then
        return nil
      end
      if part ~= "SHIFT" then
        strong = true
      end
    elseif MODIFIERS[part] or part == "ESCAPE" then
      return nil
    end
  end
  if not strong then
    return nil
  end
  return table.concat(parts, " + ")
end

-- Texto do arquivo → { dictate = { bind, keycodes }, ... }, ou nil se qualquer
-- linha fugir do formato.
local function parse(text)
  if type(text) ~= "string" or #text > MAX_BYTES then
    return nil
  end
  local found, binds, version = {}, {}, false
  for line in (text .. "\n"):gmatch("([^\n]*)\n") do
    if line == "version 1" then
      version = true
    elseif line ~= "" and line:sub(1, 1) ~= "#" then
      local name, combo, codes = line:match("^([%l_]+) ([%u%d_+]+)(.*)$")
      if not name or not ACTIONS[name] or found[name] then
        return nil
      end
      local bind = parse_combo(combo)
      if not bind or binds[bind] then
        return nil
      end
      binds[bind] = true
      if codes ~= "" and not codes:match("^ %d+[ %d]*$") then
        return nil
      end
      local keycodes = {}
      for code in codes:gmatch("%d+") do
        local number = tonumber(code)
        if #code > 3 or number < 9 or number > 255 or #keycodes >= 9 then
          return nil
        end
        keycodes[#keycodes + 1] = number
      end
      found[name] = { bind = bind, keycodes = keycodes }
    end
  end
  if not (version and found.dictate and found.hands_free and found.paste_last) then
    return nil
  end
  if #found.dictate.keycodes == 0 then
    return nil
  end
  return found
end

local function read_hotkeys()
  local file = io.open(hotkeys_file, "r")
  if not file then
    return nil
  end
  local text = file:read(MAX_BYTES + 1)
  file:close()
  return text
end

local hotkeys = parse(read_hotkeys()) or parse(DEFAULTS)
local releases = {}
for _, code in ipairs(hotkeys.dictate.keycodes) do
  releases[code] = true
end

local function ctl(action)
  hl.exec_cmd(cli .. " ctl " .. action)
end

-- Recarregar a config (inclusive ao mudar um atalho) não pode deixar uma
-- gravação "segurada" para trás, nem colar texto sem o usuário soltar: a
-- gravação segurada é cancelada.
local previous = _G.fastwisper
if previous then
  if previous.hold then
    previous.hold:remove()
    ctl("cancel")
  end
  if previous.handsfree then
    previous.handsfree:remove()
  end
end
local state = { hold = nil, handsfree = nil }
_G.fastwisper = state

local function stop_watching(which)
  if state[which] then
    state[which]:remove()
    state[which] = nil
  end
end

-- Descrições fixas em inglês: o Rust só reconhece o prefixo "FastWisper:".
-- shortcut: não seguem ui.language; para traduzir, levar o idioma no hotkeys.conf (parser estrito acima).
-- Segurando o atalho de ditar: soltar a tecla ou um modificador encerra; Esc cancela.
o.bind(hotkeys.dictate.bind, "FastWisper: dictate (hold)", function()
  if state.hold then
    return -- já gravando (repetição); o daemon também é idempotente
  end
  state.hold = hl.on("input.keyboard.key", function(keycode, _, key_state)
    if key_state == PRESSED and keycode == KEY_ESC then
      stop_watching("hold")
      ctl("cancel")
    elseif key_state == RELEASED and releases[keycode] then
      stop_watching("hold")
      ctl("release")
    end
  end)
  ctl("press")
end)

-- Mãos livres: o atalho liga e desliga; Esc cancela enquanto grava.
o.bind(hotkeys.hands_free.bind, "FastWisper: hands-free", function()
  if state.handsfree then
    stop_watching("handsfree")
  else
    state.handsfree = hl.on("input.keyboard.key", function(keycode, _, key_state)
      if key_state == PRESSED and keycode == KEY_ESC then
        stop_watching("handsfree")
        ctl("cancel")
      end
    end)
  end
  ctl("toggle")
end)

o.bind(hotkeys.paste_last.bind, "FastWisper: paste again", function()
  ctl("paste-last")
end)
