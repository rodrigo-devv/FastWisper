-- Hyprland falso para o teste de atalhos: carrega o hyprland.lua instalado
-- com `hl` e `o` simulados e executa uma sequência de passos. Nada é
-- executado de verdade: `hl.exec_cmd` só imprime o comando.
--
--   lua fake-hyprland.lua <hyprland.lua> passo...
--     binds              imprime "bind <teclas> = <descrição>" (ordenado)
--     press <teclas>     dispara o bind (ex.: "ALT + SPACE")
--     key <code> <0|1>   evento input.keyboard.key (0 solta, 1 aperta)
--     reload             recarrega o arquivo no mesmo estado Lua (como o Hyprland)
--     watchers           imprime quantos observadores de tecla estão ativos
local path = assert(arg[1], "uso: fake-hyprland.lua <hyprland.lua> passo...")
local binds, watchers = {}, {}

-- O arquivo de atalhos é só dados: qualquer execução vinda dele é falha.
os.execute = function() error("os.execute chamado") end
io.popen = function() error("io.popen chamado") end
load = function() error("load chamado") end
loadstring = load
dofile_real = dofile

hl = {
  exec_cmd = function(command) print("exec " .. command) end,
  on = function(event, callback)
    assert(event == "input.keyboard.key", "evento inesperado: " .. tostring(event))
    local watcher = { callback = callback, active = true }
    function watcher:remove() self.active = false end
    watchers[#watchers + 1] = watcher
    return watcher
  end,
  bind = function(keys, dispatcher, options)
    assert(type(dispatcher) == "function", "dispatcher precisa ser função Lua")
    assert(not options.release, "bind de release proibido")
    assert(binds[keys] == nil, "bind duplicado: " .. keys)
    binds[keys] = { run = dispatcher, description = options.description }
  end,
  dsp = { exec_cmd = function(command) return command end },
}
o = {
  bind = function(keys, description, dispatcher, options)
    local opts = options or {}
    opts.description = description
    hl.bind(keys, dispatcher, opts)
  end,
}

local function load_config()
  binds = {}
  dofile_real(path)
end

load_config()
local i = 2
while arg[i] do
  local step = arg[i]
  if step == "binds" then
    local keys = {}
    for k in pairs(binds) do keys[#keys + 1] = k end
    table.sort(keys)
    for _, k in ipairs(keys) do print("bind " .. k .. " = " .. binds[k].description) end
  elseif step == "press" then
    i = i + 1
    local bind = assert(binds[arg[i]], "sem bind: " .. tostring(arg[i]))
    bind.run()
  elseif step == "key" then
    local code, state = tonumber(arg[i + 1]), tonumber(arg[i + 2])
    i = i + 2
    for _, watcher in ipairs(watchers) do
      if watcher.active then watcher.callback(code, 0, state) end
    end
  elseif step == "reload" then
    load_config()
  elseif step == "watchers" then
    local active = 0
    for _, watcher in ipairs(watchers) do
      if watcher.active then active = active + 1 end
    end
    print("watchers " .. active)
  else
    error("passo desconhecido: " .. step)
  end
  i = i + 1
end
