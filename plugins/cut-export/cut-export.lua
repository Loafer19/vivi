-- @name Cut & Export Clip
-- @version 1.0.0
-- @author Vivi
-- @description Cut a fragment of the current file into a new file with ffmpeg (Vivi plugin package).
--
-- Keys: Alt+[   set In at the current position
--       Alt+]   set Out at the current position
--       Alt+w   export the clip (In..Out, else the A-B loop points)
-- Script messages: set-in, set-out, use-ab, clear, export, cancel, detect, pick-folder.
--
-- Range: In/Out set here win; without them the A-B loop points (ab-loop-a / ab-loop-b)
-- are used, else the whole file. "Stream copy" is fast and lossless but starts at the
-- keyframe before In; "Re-encode" is frame-accurate (H.264 + AAC through libx264 when the
-- ffmpeg has it). Vivi's media ffmpeg build has no video encoders, so re-encoding video
-- needs a system ffmpeg (otherwise stream copy is used, with a notice); audio-only files
-- re-encode to AAC with either.
--
-- ffmpeg: a system ffmpeg is used first; else Vivi's shared media ffmpeg build
-- (release helper-ffmpeg-v8.0-2, about 15 MB, sha256 pinned below), downloaded once into
-- plugin-data/shared/ffmpeg-media and shared with Chapter Thumbnails and Subtitle Auto-Sync.
local mp = require "mp"
local options = require "mp.options"
local utils = require "mp.utils"

local o = { mode = "Stream copy (fast, keyframe)", folder = "", ["in"] = "", out = "", download = true }
options.read_options(o, "cut-export")

-- ---------- managed helpers (same block in subtitle-sync, chapter-thumbnails, acoustid, cut-export) ----------
-- A system program is always preferred. Otherwise a pinned build is downloaded once
-- (curl), checked against the sha256 in this file and kept under
-- $XDG_DATA_HOME/vivi/plugin-data/<dir>. Vivi's media ffmpeg build (release
-- helper-ffmpeg-v<build>, ffmpeg-media + ffprobe-media) is shared by the plugins that
-- need video decoding: plugin-data/shared/ffmpeg-media.
local windows = package.config:sub(1, 1) == "\\"
local SEP = windows and "\\" or "/"
local EXE = windows and ".exe" or ""
local HOME = os.getenv("HOME") or os.getenv("USERPROFILE") or "."
local DATA_HOME = os.getenv("XDG_DATA_HOME")
if not DATA_HOME or DATA_HOME == "" then
  DATA_HOME = windows and (os.getenv("APPDATA") or HOME) or (HOME .. "/.local/share")
end
local PLUGIN_DATA = DATA_HOME .. SEP .. "vivi" .. SEP .. "plugin-data"
local FLATPAK = utils.file_info("/.flatpak-info") ~= nil

local function run(args, cb, extra)
  local t = { name = "subprocess", args = args, playback_only = false, capture_stdout = true, capture_stderr = true }
  for k, v in pairs(extra or {}) do t[k] = v end
  return mp.command_native_async(t, function(_, r) cb(r or {}) end)
end

local function mkdir(dir, cb)
  if windows then run({ "cmd", "/c", "if not exist \"" .. dir .. "\" mkdir \"" .. dir .. "\"" }, cb)
  else run({ "mkdir", "-p", dir }, cb) end
end

local function human(n)
  if not n then return "?" end
  if n >= 1048576 then return string.format("%.1f MB", n / 1048576) end
  return string.format("%.0f KB", n / 1024)
end

local function sha256(file, cb)
  local function first_hex(s)
    for line in (s or ""):gmatch("[^\r\n]+") do
      local h = line:gsub("%s", ""):lower():match("^(%x+)")
      if h and #h >= 64 then return h:sub(1, 64) end
    end
  end
  if windows then
    return run({ "certutil", "-hashfile", file, "SHA256" }, function(r)
      cb(first_hex((r.stdout or ""):gsub("^[^\n]*\n", "")))
    end)
  end
  run({ "sha256sum", file }, function(r)
    if r.status == 0 then return cb(first_hex(r.stdout)) end
    run({ "shasum", "-a", "256", file }, function(r2) cb(r2.status == 0 and first_hex(r2.stdout) or nil) end)
  end)
end

-- "linux-x86_64", "linux-aarch64", "macos-aarch64", "macos-x86_64", "windows-x86_64", …
local platform_cache
local function platform(cb)
  if platform_cache then return cb(platform_cache) end
  if windows then
    local a = (os.getenv("PROCESSOR_ARCHITEW6432") or os.getenv("PROCESSOR_ARCHITECTURE") or ""):upper()
    platform_cache = a == "ARM64" and "windows-aarch64" or "windows-x86_64"
    return cb(platform_cache)
  end
  run({ "uname", "-sm" }, function(r)
    local s = r.stdout or ""
    local arch = (s:match("x86_64") and "x86_64") or ((s:match("aarch64") or s:match("arm64")) and "aarch64") or "unknown"
    platform_cache = (s:match("Darwin") and "macos-" or s:match("Linux") and "linux-" or "other-") .. arch
    cb(platform_cache)
  end)
end

-- First runnable program called one of `names` in PATH or the usual folders
-- (never inside the Flatpak, whose PATH has no host tools): cb(path | nil, output).
local function find_program(names, args, cb)
  if FLATPAK or os.getenv("VIVI_TEST_NO_SYSTEM_TOOLS") then return cb(nil) end
  local list, seen = {}, {}
  local function add(dir)
    if not dir or dir == "" or dir:find(PLUGIN_DATA, 1, true) then return end
    for _, n in ipairs(names) do
      local p = dir .. SEP .. n .. EXE
      if not seen[p] and utils.file_info(p) then seen[p] = true list[#list + 1] = p end
    end
  end
  for d in (os.getenv("PATH") or ""):gmatch(windows and "[^;]+" or "[^:]+") do add(d) end
  if not windows then
    for _, d in ipairs({ HOME .. "/.local/bin", HOME .. "/.cargo/bin", "/usr/local/bin", "/usr/bin",
                         "/opt/homebrew/bin", "/opt/local/bin", "/snap/bin" }) do add(d) end
  end
  local i = 0
  local function nxt()
    i = i + 1
    if not list[i] then return cb(nil) end
    local a = { list[i] }
    for _, x in ipairs(args) do a[#a + 1] = x end
    run(a, function(r)
      if r.status == 0 then cb(list[i], (r.stdout or "") .. (r.stderr or "")) else nxt() end
    end)
  end
  nxt()
end

local function remote_size(url, cb)
  run({ "curl", "-sIL", url }, function(r)
    local n
    for v in (r.stdout or ""):gmatch("[Cc]ontent%-[Ll]ength:%s*(%d+)") do n = tonumber(v) end
    cb(n)
  end)
end

-- Download `url` into `dir`, verify `sha`; archives (.zip/.tar.gz, `unpack = true`) are
-- unpacked there with tar (bsdtar on Windows 10+ reads zip too). cb(file | nil, err)
local function fetch_verified(what, url, sha, dir, name, unpack, cb)
  mkdir(dir, function()
    local tmp = dir .. SEP .. name .. ".part"
    remote_size(url, function(sz)
      mp.osd_message("Downloading " .. what .. " (" .. human(sz) .. ")…", 5)
      mp.commandv("script-message", "vivi-toast", "Downloading " .. what .. " (" .. human(sz) .. ")…")
      run({ "curl", "-fsSL", "--retry", "2", "-o", tmp, url }, function(r)
        if r.status ~= 0 then os.remove(tmp) return cb(nil, what .. " download failed: " .. (r.stderr or "")) end
        sha256(tmp, function(got)
          if not got then os.remove(tmp) return cb(nil, "no sha256 tool to verify " .. what) end
          if got ~= sha then os.remove(tmp) return cb(nil, what .. " checksum mismatch, not installed") end
          local dst = dir .. SEP .. name
          if unpack then
            run({ "tar", "-xf", tmp, "-C", dir }, function(t)
              os.remove(tmp)
              if t.status ~= 0 then return cb(nil, "could not unpack " .. what .. ": " .. (t.stderr or "")) end
              cb(dst)
            end)
            return
          end
          os.remove(dst)
          local ok, e = os.rename(tmp, dst)
          if not ok then return cb(nil, e) end
          if windows then return cb(dst) end
          run({ "chmod", "755", dst }, function() cb(dst) end)
        end)
      end)
    end)
  end)
end

-- Vivi's media ffmpeg build (helpers/ffmpeg/build.sh FLAVOR=media).
local FF_MEDIA = {
  tag = "helper-ffmpeg-v8.0-2",
  base = "https://github.com/Loafer19/vivi/releases/download/",
  sha = {
      ["ffmpeg-media-linux-aarch64"] = "a356a2facd9a1bdf420a3dbc5edee6ce7480dff90b0f058dcb7d6259cd6fc707",
      ["ffmpeg-media-linux-x86_64"] = "64cb79d4007f78afaaf3f7cdd126dbc8937857efb4724741296aedba90eb264e",
      ["ffmpeg-media-macos-aarch64"] = "39b1c49229fd237e84e85d9c175a71f05ae4597a1253c2e060a85ac8e78b9548",
      ["ffmpeg-media-windows-x86_64.exe"] = "62d2b260aa526213f4533311e490fc83155d560244ce53d65ba7e5428d5a3f35",
      ["ffprobe-media-linux-aarch64"] = "fad8553efe177d15e95ba79e46347c03172d9e1c4bda309cb9468886a3d3f2e4",
      ["ffprobe-media-linux-x86_64"] = "e1fe69e50aaf4bf82cc41b610a2f3b45cfd37a3735a9516f8cb2d88d38385264",
      ["ffprobe-media-macos-aarch64"] = "03e3f0357a56c6984c6759d3b90c3c2e9b9a7e3384ae5b7a0480572aa731cbe1",
      ["ffprobe-media-windows-x86_64.exe"] = "944b6f8e40d3060023bf9a3faa3ca0878f8fb0e19d1784d5d91454a1545404cc",
  },
}
local FF_DIR = PLUGIN_DATA .. SEP .. "shared" .. SEP .. "ffmpeg-media"

local ff_cache, ff_wait = nil, nil
-- ffmpeg (+ ffprobe) with video decoders: { ffmpeg, ffprobe, kind }. With `need`, the
-- media build is downloaded when there is no system one. cb(ff | nil, err)
local function get_ffmpeg(need, cb)
  if ff_cache then return cb(ff_cache) end
  if ff_wait then ff_wait[#ff_wait + 1] = { need, cb } return end
  ff_wait = { { need, cb } }
  local function finish(f, err)
    ff_cache = f
    local w = ff_wait
    ff_wait = nil
    for _, x in ipairs(w) do x[2](f, err) end
  end
  local managed = { ffmpeg = FF_DIR .. SEP .. "ffmpeg" .. EXE, ffprobe = FF_DIR .. SEP .. "ffprobe" .. EXE, kind = "managed" }
  find_program({ "ffmpeg" }, { "-hide_banner", "-version" }, function(p)
    if p then
      local dir = p:match("^(.*)[/\\][^/\\]+$")
      local probe = dir .. SEP .. "ffprobe" .. EXE
      return finish({ ffmpeg = p, ffprobe = utils.file_info(probe) and probe or "ffprobe", kind = "system" })
    end
    if utils.file_info(managed.ffmpeg) and utils.file_info(managed.ffprobe) then return finish(managed) end
    local any = false
    for _, x in ipairs(ff_wait) do any = any or x[1] end
    if not any then return finish(nil, "no ffmpeg") end
    platform(function(plat)
      local files = {}
      for _, prog in ipairs({ "ffmpeg", "ffprobe" }) do
        local asset = prog .. "-media-" .. plat .. EXE
        if not FF_MEDIA.sha[asset] then return finish(nil, "no ffmpeg build for " .. plat .. " yet; install ffmpeg") end
        files[#files + 1] = { prog, asset }
      end
      local function nxt(i)
        local f = files[i]
        if not f then return finish(managed) end
        fetch_verified("ffmpeg (" .. f[1] .. ")", FF_MEDIA.base .. FF_MEDIA.tag .. "/" .. f[2], FF_MEDIA.sha[f[2]],
          FF_DIR, f[1] .. EXE, false, function(ok, e)
            if not ok then return finish(nil, e) end
            nxt(i + 1)
          end)
      end
      nxt(1)
    end)
  end)
end
-- ---------- end of managed helpers ----------

local function toast(msg, secs)
  mp.osd_message(msg, secs or 4)
  mp.commandv("script-message", "vivi-toast", msg)
end

-- "1:02:03.5", "62:03", "3723.5" -> seconds (nil when empty / invalid).
local function parse_time(s)
  s = (s or ""):gsub("^%s+", ""):gsub("%s+$", "")
  if s == "" then return nil end
  local n, total = 0, 0
  for part in s:gmatch("[^:]+") do
    local v = tonumber(part)
    if not v then return nil end
    total = total * 60 + v
    n = n + 1
  end
  if n == 0 or n > 3 or total < 0 then return nil end
  return total
end

local function fmt_time(t)
  if not t then return "--" end
  local h = math.floor(t / 3600)
  local m = math.floor(t % 3600 / 60)
  local s = t - h * 3600 - m * 60
  if h > 0 then return string.format("%d:%02d:%06.3f", h, m, s) end
  return string.format("%d:%06.3f", m, s)
end

-- Range to cut: explicit In/Out first, then the A-B loop, else the whole file.
local function range()
  local a, b = parse_time(o["in"]), parse_time(o.out)
  local src = "In/Out"
  if not a and not b then
    local la, lb = mp.get_property_native("ab-loop-a"), mp.get_property_native("ab-loop-b")
    if type(la) == "number" and type(lb) == "number" then a, b, src = la, lb, "A-B loop" end
  end
  a = a or 0
  b = b or mp.get_property_number("duration")
  if b and b < a then a, b = b, a end
  return a, b, src
end

-- /v/movie.mkv, 62.5, 70 -> <dir or /v>/movie [1m02.5s-1m10.0s].mkv
local function out_path(src, a, b, dir, reencode, audio_only)
  local srcdir, name = src:match("^(.*)[/\\]([^/\\]+)$")
  if not name then srcdir, name = ".", src end
  local stem, ext = name:match("^(.*)%.([^.]+)$")
  if not stem then stem, ext = name, "mkv" end
  if reencode then ext = audio_only and "m4a" or "mp4" end
  local function tag(t) return string.format("%dm%04.1fs", math.floor(t / 60), t % 60) end
  return (dir ~= "" and dir or srcdir) .. SEP .. stem .. " [" .. tag(a) .. "-" .. tag(b) .. "]." .. ext
end

local function ffmpeg_args(ff, src, a, b, out, reencode, audio_only)
  local args = { ff, "-hide_banner", "-loglevel", "error", "-y",
    "-ss", string.format("%.3f", a), "-i", src, "-t", string.format("%.3f", b - a) }
  local rest
  if not reencode then
    rest = { "-map", "0", "-c", "copy", "-avoid_negative_ts", "make_zero" }
  elseif audio_only then
    rest = { "-map", "0:a", "-vn", "-c:a", "aac", "-b:a", "192k" }
  else
    -- Input seek + decoding is frame-accurate.
    rest = { "-map", "0:v:0", "-map", "0:a?", "-c:v", "libx264", "-crf", "18", "-preset", "veryfast",
      "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart" }
  end
  for _, x in ipairs(rest) do args[#args + 1] = x end
  args[#args + 1] = out
  return args
end

local job, busy = nil, false

local function is_audio_only()
  local v = mp.get_property_native("current-tracks/video")
  return type(v) ~= "table" or v.image == true or v.albumart == true
end

local function export()
  if busy then return toast("An export is already running (Cancel stops it)") end
  local src = mp.get_property("path")
  if not src or src:find("://") then return toast("Cut & Export works on local files only") end
  local a, b, from = range()
  if not b then return toast("Set an Out point (the duration is unknown)") end
  if b - a < 0.05 then return toast("The clip is empty: set In before Out") end
  local reencode = (o.mode or ""):match("^Re%-encode") ~= nil
  local audio_only = is_audio_only()
  local dir = (o.folder or ""):gsub("^%s+", ""):gsub("%s+$", ""):gsub("^~", HOME)
  busy = true
  get_ffmpeg(o.download, function(ff, err)
    if not ff then busy = false return toast("Cut & Export needs ffmpeg: " .. tostring(err or "install ffmpeg"), 8) end
    local function go(x264)
      if reencode and not audio_only and not x264 then
        toast("This ffmpeg has no H.264 encoder (Vivi's media build has no video encoders), so the clip is stream-copied. Install ffmpeg for re-encoding.", 7)
        reencode = false
      end
      local out = out_path(src, a, b, dir, reencode, audio_only)
      local function start()
        toast(string.format("Exporting %s → %s (%s, %s)…", from, out:match("([^/\\]+)$"), fmt_time(b - a),
          reencode and "re-encode" or "stream copy"), 4)
        local t0 = mp.get_time()
        job = mp.command_native_async({ name = "subprocess", playback_only = false, capture_stderr = true,
          args = ffmpeg_args(ff.ffmpeg, src, a, b, out, reencode, audio_only) }, function(_, r)
          r = r or {}
          job, busy = nil, false
          if r.killed_by_us then os.remove(out) return toast("Export cancelled") end
          local fi = utils.file_info(out)
          if r.status ~= 0 or not fi then
            os.remove(out)
            local e = (r.stderr or ""):gsub("%s+$", ""):match("([^\n]*)$") or ""
            return toast("Export failed" .. (e ~= "" and (": " .. e:sub(1, 140)) or ""), 8)
          end
          toast(string.format("Saved %s (%s, %.0f s)", out, human(fi.size), mp.get_time() - t0), 7)
        end)
      end
      if dir ~= "" then mkdir(dir, start) else start() end
    end
    if not reencode or audio_only then return go(false) end
    run({ ff.ffmpeg, "-hide_banner", "-encoders" }, function(r) go((r.stdout or ""):find("libx264", 1, true) ~= nil) end)
  end)
end

local function remember(key, v)
  o[key] = v
  mp.commandv("script-message", "vivi-setting", "cut-export", key, v)
end

local function set_point(key, label)
  return function()
    local t = mp.get_property_number("time-pos")
    if not t then return end
    remember(key, fmt_time(t))
    local a, b = range()
    toast(string.format("%s %s (clip %s – %s)", label, o[key], fmt_time(a), fmt_time(b)), 2)
  end
end

local function use_ab()
  local la, lb = mp.get_property_native("ab-loop-a"), mp.get_property_native("ab-loop-b")
  if type(la) ~= "number" or type(lb) ~= "number" then return toast("No A-B loop set (Alt+A / Alt+B with A-B Loop)") end
  remember("in", fmt_time(la))
  remember("out", fmt_time(lb))
  toast("Clip set from the A-B loop: " .. o["in"] .. " – " .. o.out, 3)
end

local function clear(quiet)
  if (o["in"] or "") == "" and (o.out or "") == "" then
    if not quiet then toast("In/Out are already empty") end
    return
  end
  remember("in", "")
  remember("out", "")
  if not quiet then toast("In/Out cleared (the A-B loop is used if set)", 3) end
end

local function detect()
  ff_cache = nil
  get_ffmpeg(false, function(ff)
    toast("ffmpeg: " .. (ff and (ff.ffmpeg .. " (" .. ff.kind .. ")")
      or "none yet (Vivi's media build, about 15 MB, is downloaded on the first export)"), 8)
  end)
end

options.read_options(o, "cut-export", function() end)
mp.add_forced_key_binding("Alt+[", "cut-in", set_point("in", "In"))
mp.add_forced_key_binding("Alt+]", "cut-out", set_point("out", "Out"))
mp.add_forced_key_binding("Alt+w", "cut-export", export)
mp.register_script_message("set-in", set_point("in", "In"))
mp.register_script_message("set-out", set_point("out", "Out"))
mp.register_script_message("use-ab", use_ab)
mp.register_script_message("clear", function() clear(false) end)
mp.register_script_message("export", export)
mp.register_script_message("cancel", function()
  if job then mp.abort_async_command(job) else toast("No export is running") end
end)
mp.register_script_message("detect", detect)
mp.register_script_message("pick-folder", function()
  mp.commandv("script-message", "vivi-pick-folder", "cut-export", "folder")
end)
mp.register_script_message("folder-picked", function(p) o.folder = p toast("Clips go to " .. p) end)
-- In/Out belong to one file: forget them when it ends.
mp.register_event("end-file", function() clear(true) end)

if VIVI_TEST then
  return { parse_time = parse_time, fmt_time = fmt_time, range = range, out_path = out_path,
    ffmpeg_args = ffmpeg_args, FF_MEDIA = FF_MEDIA, o = o }
end
