-- @name A-B Loop & Frame Step
-- @version 1.0.0
-- @author Vivi
-- @description A-B loop editing and frame stepping (Vivi plugin package).
--
-- Keys (binding names for script-binding ab_loop/<name>):
--   Alt+a  set-a     set A at the current position
--   Alt+b  set-b     set B at the current position
--   Alt+c  clear     clear the loop
--   Alt+Left / Alt+Right   nudge the last set point by one frame
--   ,  .   step one frame back / forward (only when paused; otherwise seek 1 s)
local mp = require "mp"
local options = require "mp.options"

local o = { markers = true, ["osd-seconds"] = 3 }
options.read_options(o, "ab-loop")

local overlay = mp.create_osd_overlay("ass-events")
local hide_timer
local last = "a"

local function num(p)
  local v = mp.get_property_native(p)
  if type(v) == "number" then return v end
  return nil
end

local function fmt(t)
  if not t then return "--:--" end
  return string.format("%d:%05.2f", math.floor(t / 60), t % 60)
end

local function draw()
  if not o.markers then overlay:remove(); return end
  local a, b = num("ab-loop-a"), num("ab-loop-b")
  local dur = num("duration")
  if (not a and not b) or not dur or dur <= 0 then overlay:remove(); return end
  local w, h = 1000, 1000
  overlay.res_x, overlay.res_y = w, h
  local x0, x1, y = 100, 900, 900
  local function px(t) return x0 + (x1 - x0) * math.max(0, math.min(1, t / dur)) end
  local ev = {}
  -- track
  ev[#ev + 1] = string.format("{\\an7\\pos(0,0)\\bord0\\shad0\\1c&H808080&\\1a&H80&\\p1}m %d %d l %d %d %d %d %d %d{\\p0}",
    x0, y, x1, y, x1, y + 6, x0, y + 6)
  local pa, pb = a and px(a) or x0, b and px(b) or px(num("time-pos") or 0)
  ev[#ev + 1] = string.format("{\\an7\\pos(0,0)\\bord0\\shad0\\1c&H00C8FF&\\1a&H20&\\p1}m %d %d l %d %d %d %d %d %d{\\p0}",
    pa, y - 4, pb, y - 4, pb, y + 10, pa, y + 10)
  ev[#ev + 1] = string.format("{\\an2\\pos(%d,%d)\\fs22\\bord2}A %s", pa, y - 8, fmt(a))
  if b then ev[#ev + 1] = string.format("{\\an2\\pos(%d,%d)\\fs22\\bord2}B %s", pb, y - 34, fmt(b)) end
  overlay.data = table.concat(ev, "\n")
  overlay:update()
  if hide_timer then hide_timer:kill() end
  hide_timer = mp.add_timeout(tonumber(o["osd-seconds"]) or 3, function() overlay:remove() end)
end

local function set(which)
  local t = num("time-pos")
  if not t then return end
  mp.set_property_native("ab-loop-" .. which, t)
  last = which
  mp.osd_message(string.upper(which) .. ": " .. fmt(t))
  draw()
end

local function clear()
  mp.set_property("ab-loop-a", "no")
  mp.set_property("ab-loop-b", "no")
  overlay:remove()
  mp.osd_message("A-B loop cleared")
end

local function nudge(dir)
  local p = "ab-loop-" .. last
  local t = num(p)
  if not t then return end
  local fps = num("container-fps") or num("estimated-vf-fps") or 25
  mp.set_property_native(p, math.max(0, t + dir / fps))
  mp.osd_message(string.upper(last) .. ": " .. fmt(num(p)))
  draw()
end

local function step(dir)
  if mp.get_property_native("pause") then
    mp.command(dir > 0 and "frame-step" or "frame-back-step")
  else
    mp.commandv("seek", tostring(dir), "relative", "exact")
  end
end

mp.add_forced_key_binding("Alt+a", "set-a", function() set("a") end)
mp.add_forced_key_binding("Alt+b", "set-b", function() set("b") end)
mp.add_forced_key_binding("Alt+c", "clear", clear)
mp.add_forced_key_binding("Alt+LEFT", "nudge-back", function() nudge(-1) end, { repeatable = true })
mp.add_forced_key_binding("Alt+RIGHT", "nudge-forward", function() nudge(1) end, { repeatable = true })
mp.add_forced_key_binding(",", "frame-back", function() step(-1) end, { repeatable = true })
mp.add_forced_key_binding(".", "frame-forward", function() step(1) end, { repeatable = true })
-- Vivi's "Clear loop" button sends script-message-to ab-loop clear.
mp.register_script_message("clear", clear)
mp.observe_property("ab-loop-a", "native", function() draw() end)
mp.observe_property("ab-loop-b", "native", function() draw() end)
