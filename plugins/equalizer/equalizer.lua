-- @name Equalizer
-- @version 1.0.0
-- @author Vivi
-- @description Equalizer presets and a custom 10-band EQ through mpv audio filters (Vivi plugin package).
--
-- Keys: Alt+e  next preset    Alt+E  equalizer on / off
-- Script messages: next, toggle, show.
--
-- The EQ is one labelled mpv filter, @vivi-eqplug (lavfi), so it never touches Vivi's own
-- Audio-panel EQ (@vivi-eq) or other filters. Engines:
--   * equalizer      ten ffmpeg `equalizer` peaking filters (one octave wide) at
--                    31 Hz … 16 kHz, gains in dB;
--   * superequalizer ffmpeg's 18-band FIR equalizer; the ten gains are interpolated on a
--                    log-frequency scale and passed as linear factors.
-- A pre-amp of minus the largest boost keeps boosted presets from clipping (optional).
-- The preset, gains and on/off state are plugin settings, so they persist across
-- restarts; Alt+e / Alt+E save them with `script-message vivi-setting`.
local mp = require "mp"
local options = require "mp.options"

local ID = "equalizer"
local LABEL = "@vivi-eqplug"
local FREQS = { 31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000 }
local PRESETS = {
  Flat  = { 0, 0, 0, 0, 0, 0, 0, 0, 0, 0 },
  Bass  = { 6, 5, 4, 2, 0, 0, 0, 0, 0, 0 },
  Vocal = { -2, -2, -1, 1, 3, 4, 4, 2, 0, -1 },
  Rock  = { 5, 3, 1, -1, -2, -1, 1, 3, 4, 5 },
  Night = { -6, -5, -3, -1, 1, 2, 2, 1, 0, -1 },
}
local ORDER = { "Flat", "Bass", "Vocal", "Rock", "Night", "Custom" }
-- superequalizer band centres (Hz), 1b … 18b.
local SUPER = { 65, 92, 131, 185, 262, 370, 523, 740, 1047, 1480, 2093, 2960, 4186, 5920, 8372, 11840, 16744, 20000 }

local o = {
  enabled = true,
  preset = "Flat",
  custom = "0 0 0 0 0 0 0 0 0 0",
  engine = "equalizer (10 bands)",
  preamp = true,
}

-- Ten gains (dB, clamped to ±12) from "a b c …" (commas allowed); missing bands are 0.
local function parse_gains(s)
  local g = {}
  for v in (s or ""):gmatch("[-+]?%d*%.?%d+") do
    if #g < 10 then g[#g + 1] = math.max(-12, math.min(12, tonumber(v) or 0)) end
  end
  for i = #g + 1, 10 do g[i] = 0 end
  return g
end

local function gains()
  if o.preset == "Custom" then return parse_gains(o.custom) end
  return PRESETS[o.preset] or PRESETS.Flat
end

local function fmt(x) return (string.format("%.2f", x):gsub("0+$", ""):gsub("%.$", "")) end

-- Gain (dB) at frequency f by linear interpolation over log2(frequency).
local function gain_at(g, f)
  local lf = math.log(f) / math.log(2)
  if f <= FREQS[1] then return g[1] end
  if f >= FREQS[10] then return g[10] end
  for i = 1, 9 do
    local a, b = FREQS[i], FREQS[i + 1]
    if f >= a and f <= b then
      local la, lb = math.log(a) / math.log(2), math.log(b) / math.log(2)
      return g[i] + (g[i + 1] - g[i]) * (lf - la) / (lb - la)
    end
  end
  return 0
end

-- lavfi graph for the gains, or nil when flat.
local function graph(g, engine, preamp)
  local flat, max = true, 0
  for _, v in ipairs(g) do
    if v ~= 0 then flat = false end
    if v > max then max = v end
  end
  if flat then return nil end
  local parts = {}
  if engine:find("^superequalizer") then
    local b = {}
    for i, f in ipairs(SUPER) do b[#b + 1] = i .. "b=" .. fmt(10 ^ (gain_at(g, f) / 20)) end
    parts[1] = "superequalizer=" .. table.concat(b, ":")
  else
    for i, f in ipairs(FREQS) do
      if g[i] ~= 0 then parts[#parts + 1] = string.format("equalizer=f=%d:t=o:w=1:g=%s", f, fmt(g[i])) end
    end
  end
  if preamp and max > 0 then parts[#parts + 1] = "volume=" .. fmt(-max) .. "dB" end
  return table.concat(parts, ",")
end

local function describe(g)
  local s = {}
  for i, v in ipairs(g) do s[i] = (v > 0 and "+" or "") .. fmt(v) end
  return table.concat(s, " ")
end

local applied = nil

local function apply(announce)
  local gr = o.enabled and graph(gains(), o.engine, o.preamp) or nil
  if gr ~= applied then
    if applied then mp.commandv("af", "remove", LABEL) end
    if gr then mp.commandv("af", "add", LABEL .. ":lavfi=[" .. gr .. "]") end
    applied = gr
    mp.msg.info("af " .. (gr or "off"))
  end
  if announce then
    local msg = not o.enabled and "Equalizer off"
      or ("Equalizer: " .. o.preset .. (o.preset == "Custom" and (" (" .. describe(gains()) .. ")") or ""))
    mp.osd_message(msg, 2)
    mp.commandv("script-message", "vivi-toast", msg)
  end
end

local function save(key, value)
  mp.commandv("script-message", "vivi-setting", ID, key, value)
end

local function next_preset()
  local i = 1
  for k, v in ipairs(ORDER) do if v == o.preset then i = k end end
  o.preset = ORDER[i % #ORDER + 1]
  o.enabled = true
  save("preset", o.preset)
  save("enabled", "yes")
  apply(true)
end

local function toggle()
  o.enabled = not o.enabled
  save("enabled", o.enabled and "yes" or "no")
  apply(true)
end

options.read_options(o, ID, function() apply(false) end)
apply(false)
-- The filter chain survives file changes, but re-check in case something reset `af`.
mp.register_event("file-loaded", function()
  local af = mp.get_property("af") or ""
  if applied and not af:find("vivi%-eqplug") then applied = nil apply(false) end
end)

mp.add_forced_key_binding("Alt+e", "next", next_preset)
mp.add_forced_key_binding("Alt+E", "toggle", toggle)
mp.register_script_message("next", next_preset)
mp.register_script_message("toggle", toggle)
mp.register_script_message("show", function() apply(true) end)

if VIVI_TEST then
  return { parse_gains = parse_gains, graph = graph, gain_at = gain_at, PRESETS = PRESETS,
    o = o, apply = apply, next_preset = next_preset }
end
