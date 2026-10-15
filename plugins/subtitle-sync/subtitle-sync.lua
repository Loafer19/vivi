-- @name Subtitle Auto-Sync
-- @version 1.0.1
-- @author Vivi
-- @description Align external subtitles to the audio with alass or ffsubsync (Vivi plugin package).
--
-- Keys: Alt+y  sync   (script-binding subtitle_sync/sync)
-- Script messages: sync, detect.
local mp = require "mp"
local options = require "mp.options"
local utils = require "mp.utils"

local o = { tool = "Auto" }
options.read_options(o, "subtitle-sync")

local function toast(msg, secs)
  mp.osd_message(msg, secs or 4)
  mp.commandv("script-message", "vivi-toast", msg)
end

local running = false
local found -- cache: { alass = "alass-cli", ffsubsync = "ffsubsync" }

-- Whether a command exists and exits with status 0.
local function runs(args)
  local r = mp.command_native({ name = "subprocess", args = args, playback_only = false,
    capture_stdout = true, capture_stderr = true })
  return r ~= nil and r.status == 0
end

local function detect()
  if found then return found end
  found = {}
  for _, c in ipairs({ "alass", "alass-cli" }) do
    if runs({ c, "--version" }) then found.alass = c; break end
  end
  if runs({ "ffsubsync", "--version" }) then found.ffsubsync = "ffsubsync" end
  return found
end

local MISSING = "Subtitle sync needs alass or ffsubsync in PATH: cargo install alass-cli, or pip install ffsubsync (needs ffmpeg)"

local function pick()
  local f = detect()
  local want = (o.tool or "Auto"):lower()
  if want == "alass" then return f.alass and "alass" or nil, f end
  if want == "ffsubsync" then return f.ffsubsync and "ffsubsync" or nil, f end
  if f.alass then return "alass", f end
  if f.ffsubsync then return "ffsubsync", f end
  return nil, f
end

-- Output path next to the original: movie.en.srt -> movie.en.synced.srt
local function out_path(sub)
  local base, ext = sub:match("^(.*)%.([^./\\]+)$")
  if not base then return sub .. ".synced.srt" end
  base = base:gsub("%.synced$", "")
  return base .. ".synced." .. ext
end

local function current_sub()
  for _, t in ipairs(mp.get_property_native("track-list") or {}) do
    if t.type == "sub" and t.selected and t.external and t["external-filename"] then
      return t["external-filename"]
    end
  end
  return nil
end

local function sync()
  if running then toast("Subtitle sync is already running"); return end
  local video = mp.get_property("path")
  if not video or video:find("://") then toast("Subtitle sync works on local files only"); return end
  local sub = current_sub()
  if not sub then toast("Select an external subtitle first (embedded tracks are not supported)"); return end
  local tool, f = pick()
  if not tool then
    if (o.tool or "Auto") ~= "Auto" and (f.alass or f.ffsubsync) then
      toast(o.tool .. " is not installed; set Sync tool to Auto")
    else
      toast(MISSING, 8)
    end
    return
  end
  local out = out_path(sub)
  local args
  if tool == "alass" then
    args = { f.alass, video, sub, out }
  else
    args = { "ffsubsync", video, "-i", sub, "-o", out }
  end
  running = true
  toast("Syncing subtitle with " .. tool .. "…", 3)
  mp.msg.info("running: " .. table.concat(args, " "))
  local t0 = mp.get_time()
  mp.command_native_async({ name = "subprocess", args = args, playback_only = false,
    capture_stdout = true, capture_stderr = true }, function(ok, r)
    running = false
    if not ok or not r or r.status ~= 0 or not utils.file_info(out) then
      local err = r and (r.stderr ~= "" and r.stderr or r.stdout) or ""
      mp.msg.warn("sync failed: " .. tostring(err))
      local last = (err or ""):gsub("%s+$", ""):match("([^\n]*)$") or ""
      toast("Subtitle sync failed" .. (last ~= "" and (": " .. last:sub(1, 120)) or ""), 6)
      return
    end
    mp.commandv("sub-add", out, "select", "synced")
    local name = out:match("([^/\\]+)$")
    toast(string.format("Synced subtitle loaded: %s (%.0f s)", name, mp.get_time() - t0))
  end)
end

local function report()
  found = nil
  local f = detect()
  local list = {}
  if f.alass then list[#list + 1] = f.alass end
  if f.ffsubsync then list[#list + 1] = "ffsubsync" end
  if #list == 0 then toast(MISSING, 8) else toast("Found: " .. table.concat(list, ", ")) end
end

mp.add_forced_key_binding("Alt+y", "sync", sync)
mp.register_script_message("sync", sync)
mp.register_script_message("detect", report)
