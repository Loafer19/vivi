-- @name Playback Statistics
-- @version 1.0.1
-- @author Vivi
-- @description Local watch/listen time statistics (Vivi plugin package).
--
-- Data: $XDG_DATA_HOME/vivi/plugin-data/playback-stats/stats.tsv
-- (one line per day + item: date, seconds, kind, artist/show, title). Nothing leaves the machine.
-- Alt+S (script-binding playback_stats/show) shows a summary on the OSD.
-- Script messages: show, export, clear.

local mp = require "mp"
local options = require "mp.options"
local utils = require "mp.utils"

-- mpv's OSD is not always visible in Vivi (no video, UI on top), so messages also go
-- to Vivi's toast through `script-message vivi-toast <text>`.
local function osd(msg, secs)
  mp.osd_message(msg, secs)
  mp.commandv("script-message", "vivi-toast", msg)
end

local o = { days = 7 }
options.read_options(o, "playback-stats")

local home = os.getenv("HOME") or os.getenv("USERPROFILE") or "."
local data = os.getenv("XDG_DATA_HOME")
if not data or data == "" then data = home .. "/.local/share" end
local dir = data .. "/vivi/plugin-data/playback-stats"
local file = dir .. "/stats.tsv"

local rows = {} -- key "date\tkind\tartist\ttitle" -> seconds
local dirty = false
local current = nil -- { kind, artist, title }
local last_t = nil

local function clean(s) return (tostring(s or ""):gsub("[\t\r\n]", " ")) end

local function load()
  local f = io.open(file, "r")
  if not f then return end
  for line in f:lines() do
    local d, s, k, a, t = line:match("^([^\t]*)\t([^\t]*)\t([^\t]*)\t([^\t]*)\t(.*)$")
    if d and tonumber(s) then
      local key = table.concat({ d, k, a, t }, "\t")
      rows[key] = (rows[key] or 0) + tonumber(s)
    end
  end
  f:close()
end

local function save()
  if not dirty then return end
  utils.subprocess({ args = { "mkdir", "-p", dir }, playback_only = false })
  local tmp = file .. ".tmp"
  local f = io.open(tmp, "w")
  if not f then return end
  for key, s in pairs(rows) do
    local d, k, a, t = key:match("^([^\t]*)\t([^\t]*)\t([^\t]*)\t(.*)$")
    f:write(string.format("%s\t%d\t%s\t%s\t%s\n", d, math.floor(s + 0.5), k, a, t))
  end
  f:close()
  os.rename(tmp, file)
  dirty = false
end

local function describe()
  local md = mp.get_property_native("metadata") or {}
  local function m(k) return md[k] or md[string.upper(k)] or md[k:sub(1, 1):upper() .. k:sub(2)] end
  local video = false
  for _, tr in ipairs(mp.get_property_native("track-list") or {}) do
    if tr.type == "video" and not tr.albumart and not tr.image then video = true end
  end
  local title = m("title") or mp.get_property("media-title") or mp.get_property("filename") or "?"
  local artist = m("artist") or m("album_artist") or ""
  if video then
    -- Group episodes by show: "Show S01E02 ..." -> "Show".
    local show = m("show") or (mp.get_property("filename/no-ext") or ""):match("^(.-)[ ._-]+[Ss]%d+[Ee]%d+")
    return { "video", clean(show and show:gsub("[._]", " ") or ""), clean(title) }
  end
  return { "audio", clean(artist), clean(title) }
end

local function tick()
  local now = mp.get_time()
  if current and last_t and not mp.get_property_native("pause") and not mp.get_property_native("idle-active") then
    local dt = math.min(now - last_t, 10)
    local key = table.concat({ os.date("%Y-%m-%d"), current[1], current[2], current[3] }, "\t")
    rows[key] = (rows[key] or 0) + dt
    dirty = true
  end
  last_t = now
end

local function hms(s)
  s = math.floor(s)
  if s >= 3600 then return string.format("%dh %02dm", math.floor(s / 3600), math.floor((s % 3600) / 60)) end
  return string.format("%dm %02ds", math.floor(s / 60), s % 60)
end

local function show()
  tick()
  local days = tonumber(o.days) or 7
  local since = os.date("%Y-%m-%d", os.time() - (days - 1) * 86400)
  local total, kinds, who, per_day = 0, { audio = 0, video = 0 }, {}, {}
  for key, s in pairs(rows) do
    local d, k, a = key:match("^([^\t]*)\t([^\t]*)\t([^\t]*)\t")
    if d and d >= since then
      total = total + s
      kinds[k] = (kinds[k] or 0) + s
      if a ~= "" then who[a] = (who[a] or 0) + s end
      per_day[d] = (per_day[d] or 0) + s
    end
  end
  local top = {}
  for a, s in pairs(who) do top[#top + 1] = { a, s } end
  table.sort(top, function(x, y) return x[2] > y[2] end)
  local lines = {
    string.format("Last %d days: %s (video %s, audio %s)", days, hms(total), hms(kinds.video or 0), hms(kinds.audio or 0)),
  }
  for i = 1, math.min(5, #top) do lines[#lines + 1] = string.format("%d. %s  %s", i, top[i][1], hms(top[i][2])) end
  local maxd = 1
  for _, s in pairs(per_day) do maxd = math.max(maxd, s) end
  for i = math.min(days, 7) - 1, 0, -1 do
    local d = os.date("%Y-%m-%d", os.time() - i * 86400)
    local s = per_day[d] or 0
    lines[#lines + 1] = string.format("%s %s %s", d:sub(6), string.rep("█", math.floor(20 * s / maxd + 0.5)), hms(s))
  end
  osd(table.concat(lines, "\n"), 8)
end

local function export()
  tick()
  save()
  local out = home .. "/vivi-stats.csv"
  local f = io.open(out, "w")
  if not f then osd("Could not write " .. out); return end
  local function q(s) return '"' .. tostring(s):gsub('"', '""') .. '"' end
  f:write("date,seconds,kind,artist_or_show,title\n")
  local keys = {}
  for key in pairs(rows) do keys[#keys + 1] = key end
  table.sort(keys)
  for _, key in ipairs(keys) do
    local d, k, a, t = key:match("^([^\t]*)\t([^\t]*)\t([^\t]*)\t(.*)$")
    f:write(string.format("%s,%d,%s,%s,%s\n", d, math.floor(rows[key] + 0.5), k, q(a), q(t)))
  end
  f:close()
  osd("Statistics exported to " .. out)
end

local function clear()
  rows = {}
  dirty = false
  os.remove(file)
  osd("Playback statistics cleared")
end

load()
mp.register_event("file-loaded", function() tick(); current = describe(); last_t = mp.get_time() end)
mp.register_event("end-file", function() tick(); current = nil; save() end)
mp.observe_property("pause", "bool", function() tick() end)
mp.register_event("shutdown", function() tick(); save() end)
mp.add_periodic_timer(5, tick)
mp.add_periodic_timer(60, save)
mp.add_forced_key_binding("Alt+s", "show", show)
mp.register_script_message("show", show)
mp.register_script_message("export", export)
mp.register_script_message("clear", clear)
