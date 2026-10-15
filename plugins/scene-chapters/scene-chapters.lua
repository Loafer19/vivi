-- @name Scene Chapters & Thumbnails
-- @version 1.0.1
-- @author Vivi
-- @description Scene-change chapters and chapter thumbnails via ffmpeg (Vivi plugin package).
--
-- Keys: Alt+j  detect
-- Script messages: detect, clear.
local mp = require "mp"
local options = require "mp.options"
local utils = require "mp.utils"

local o = { auto = false, threshold = "0.4", min_gap = "30", thumbnails = true }

local function toast(msg, secs)
  mp.osd_message(msg, secs or 3)
  mp.commandv("script-message", "vivi-toast", msg)
end

local home = os.getenv("HOME") or os.getenv("USERPROFILE") or "."
local data = os.getenv("XDG_DATA_HOME")
if not data or data == "" then data = os.getenv("APPDATA") or (home .. "/.local/share") end
local root = data .. "/vivi/plugin-data/scene-chapters"
local windows = package.config:sub(1, 1) == "\\"

local busy = false
local cur_path = nil

local function mkdir(d)
  if windows then
    utils.subprocess({ args = { "cmd", "/c", "mkdir", (d:gsub("/", "\\")) }, playback_only = false })
  else
    utils.subprocess({ args = { "mkdir", "-p", d }, playback_only = false })
  end
end

-- Stable key (32-bit polynomial hash, hex) of path, size and mtime.
local function key(path)
  local fi = utils.file_info(path)
  if not fi then return nil end
  local s = path .. "|" .. tostring(fi.size) .. "|" .. tostring(math.floor(fi.mtime or 0))
  local h = 5381
  for i = 1, #s do h = (h * 33 + s:byte(i)) % 4294967296 end
  return string.format("%08x", h)
end

-- Scene times from ffmpeg showinfo output, at least `gap` seconds apart (and from 0).
local function parse_scenes(text, gap)
  local out, last = {}, 0
  for t in (text or ""):gmatch("pts_time:([%d%.]+)") do
    t = tonumber(t)
    if t and t - last >= gap then out[#out + 1] = t; last = t end
  end
  return out
end

local function read_cache(dir)
  local f = io.open(dir .. "/scenes.json", "r")
  if not f then return nil end
  local v = utils.parse_json(f:read("*a"))
  f:close()
  return type(v) == "table" and v or nil
end

local function write_cache(dir, scenes)
  mkdir(dir)
  local f = io.open(dir .. "/scenes.json", "w")
  if f then f:write(utils.format_json(scenes)); f:close() end
end

local function thumbs(path, dir, times, i)
  i = i or 1
  if not o.thumbnails or i > #times or path ~= cur_path then return end
  local out = string.format("%s/chapter-%d.jpg", dir, i)
  if utils.file_info(out) then return thumbs(path, dir, times, i + 1) end
  mp.command_native_async({ name = "subprocess", playback_only = false, capture_stderr = true,
    args = { "ffmpeg", "-nostdin", "-loglevel", "error", "-y", "-ss", string.format("%.3f", times[i] + 1),
      "-i", path, "-frames:v", "1", "-vf", "scale=320:-2", "-q:v", "4", out } },
    function() thumbs(path, dir, times, i + 1) end)
end

local function apply(scenes)
  local list = { { time = 0, title = "Scene 1" } }
  for i, t in ipairs(scenes) do list[#list + 1] = { time = t, title = "Scene " .. (i + 1) } end
  mp.set_property_native("chapter-list", list)
end

local function chapter_times()
  local t = {}
  for _, c in ipairs(mp.get_property_native("chapter-list") or {}) do t[#t + 1] = c.time end
  return t
end

local function local_video()
  local path = mp.get_property("path")
  if not path or path:find("://") or not utils.file_info(path) then return nil, "local files only" end
  local vid = false
  for _, t in ipairs(mp.get_property_native("track-list") or {}) do
    if t.type == "video" and not t.albumart and not t.image then vid = true end
  end
  if not vid then return nil, "no video track" end
  return path
end

local function detect(manual)
  local path, why = local_video()
  if not path then if manual then toast("Scene detection: " .. why) end; return end
  if busy then if manual then toast("Scene detection is already running") end; return end
  local dir = root .. "/" .. key(path)
  local has = (mp.get_property_number("chapters") or 0) > 0
  local cached = read_cache(dir)
  if cached then
    if not has then apply(cached) end
    thumbs(path, dir, chapter_times())
    if manual then toast(string.format("%d scenes (cached)", #cached + 1)) end
    return
  end
  if has and not manual then thumbs(path, dir, chapter_times()); mkdir(dir); return end
  local th = tonumber(o.threshold) or 0.4
  local gap = tonumber(o.min_gap) or 30
  busy = true
  toast("Detecting scenes…")
  mp.command_native_async({ name = "subprocess", playback_only = false, capture_stderr = true,
    capture_stdout = true,
    args = { "ffmpeg", "-nostdin", "-hide_banner", "-i", path, "-an", "-sn", "-dn",
      "-vf", string.format("scale=160:-2,select='gt(scene,%.2f)',showinfo", th), "-f", "null", "-" } },
    function(ok, r)
      busy = false
      if not r or r.error_string == "init" then
        toast("Scene detection needs ffmpeg in PATH", 6); return
      end
      if not ok then toast("Scene detection failed: " .. tostring(r.error_string)); return end
      if r.status ~= 0 then toast("Scene detection failed (ffmpeg exit " .. tostring(r.status) .. ")"); return end
      local scenes = parse_scenes(r.stderr, gap)
      mkdir(dir)
      write_cache(dir, scenes)
      if path ~= cur_path then return end
      if has then
        toast("This file has chapters; scenes cached, thumbnails only")
      else
        apply(scenes)
        toast(string.format("%d scene chapters", #scenes + 1))
      end
      thumbs(path, dir, chapter_times())
    end)
end

local function clear()
  local path = mp.get_property("path")
  local k = path and key(path)
  if not k then return end
  local dir = root .. "/" .. k
  for _, f in ipairs(utils.readdir(dir, "files") or {}) do os.remove(dir .. "/" .. f) end
  os.remove(dir)
  toast("Cached scenes removed")
end

options.read_options(o, "scene-chapters")
mp.register_event("file-loaded", function()
  cur_path = mp.get_property("path")
  if o.auto then mp.add_timeout(1, function() detect(false) end) end
end)
mp.add_forced_key_binding("Alt+j", "detect", function() detect(true) end)
mp.register_script_message("detect", function() detect(true) end)
mp.register_script_message("clear", clear)
mp.register_script_message("selftest", function()
  local s = parse_scenes("x pts_time:1.0 y pts_time:40.5 pts_time:50 pts_time:80.25", 30)
  print("SELFTEST " .. table.concat(s, ","))
end)
