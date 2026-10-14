-- @name Night Mode
-- @version 1.0.2
-- @author Vivi
-- @description Dynamic range compression for quiet listening (Vivi plugin package).
--
-- Alt+N (script-binding night_mode/toggle) switches it for this session; the
-- "Night mode on" setting in Vivi decides the state on startup and is applied live.

local mp = require "mp"
local options = require "mp.options"

-- mpv's OSD is not always visible in Vivi (no video, UI on top), so messages also go
-- to Vivi's toast through `script-message vivi-toast <text>`.
local function osd(msg, secs)
  mp.osd_message(msg, secs)
  mp.commandv("script-message", "vivi-toast", msg)
end

local o = { active = true, strength = "Medium" }
local LABEL = "@vivinight"

local FILTERS = {
  ["Light"] = "dynaudnorm=f=250:g=15:p=0.9:m=5",
  ["Medium"] = "dynaudnorm=f=150:g=11:p=0.95:m=10",
  ["Strong"] = "dynaudnorm=f=100:g=7:p=0.97:m=20:r=0.3",
  ["Loudness (EBU R128)"] = "loudnorm=I=-24:LRA=7:TP=-2",
}

local on = false

local function apply(state, quiet)
  mp.commandv("af", "remove", LABEL)
  on = state
  if on then
    local f = FILTERS[o.strength] or FILTERS["Medium"]
    mp.commandv("af", "add", LABEL .. ":lavfi=[" .. f .. "]")
  end
  if not quiet then
    osd("Night mode " .. (on and ("on (" .. o.strength .. ")") or "off"))
  end
end

options.read_options(o, "night-mode", function(changed)
  -- Vivi changed a setting while playing.
  -- Changing the strength while off leaves it off but says what will be used.
  if changed.active then
    apply(o.active)
  elseif changed.strength then
    if on then apply(true) else osd("Night mode strength: " .. o.strength) end
  end
end)

mp.add_forced_key_binding("Alt+n", "toggle", function() apply(not on) end)
mp.register_script_message("toggle", function() apply(not on) end)
apply(o.active, true)
