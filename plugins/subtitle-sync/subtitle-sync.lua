-- @name Subtitle Auto-Sync
-- @version 1.1.0
-- @author Vivi
-- @description Align external subtitles to the audio with alass or ffsubsync, plus manual offsets remembered per file (Vivi plugin package).
--
-- Keys: Alt+y           sync the selected external subtitle to the audio
--       Alt+z / Alt+x   subtitle offset -0.1 s / +0.1 s
--       Alt+Z / Alt+X   subtitle offset -1 s / +1 s   (Alt+Shift+z / x)
--       Alt+BS          reset the offset of this file
-- Script messages: sync, detect, reset.
--
-- Tools: a system alass / alass-cli or ffsubsync (PATH and the usual folders) is used
-- first. Without one, alass 2.0.0 is downloaded on first sync from its official release
-- (https://github.com/kaegi/alass/releases, sha256 pinned below): 3 MB on Linux x86_64,
-- 26 MB on Windows (bundles its ffmpeg). alass reads the audio through ffmpeg/ffprobe: a
-- system ffmpeg, else Vivi's shared media ffmpeg build (about 15 MB, also used by
-- Chapter Thumbnails). macOS and Linux ARM have no alass build: install alass-cli
-- (cargo install alass-cli) or ffsubsync (pip install ffsubsync).
--
-- Offsets: kept in plugin-data/subtitle-sync/offsets.json per file (path, size) and
-- applied as sub-delay whenever that file is opened again.
local mp = require "mp"
local options = require "mp.options"
local utils = require "mp.utils"

local o = { tool = "Auto", download = true, step = "0.1" }
options.read_options(o, "subtitle-sync")

-- ---------- managed helpers (same block in subtitle-sync, chapter-thumbnails, acoustid) ----------
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

local DIR = PLUGIN_DATA .. SEP .. "subtitle-sync"
local OFFSETS = DIR .. SEP .. "offsets.json"
local ALASS_DIR = DIR .. SEP .. "alass"
local ALASS = {
  ["linux-x86_64"] = { url = "https://github.com/kaegi/alass/releases/download/v2.0.0/alass-linux64",
    sha = "7bd0b9ae7e035d3ba940eacffb21243614df36231d47f21f0b4ce42001ab7fcd", file = "alass" },
  ["windows-x86_64"] = { url = "https://github.com/kaegi/alass/releases/download/v2.0.0/alass-windows64.zip",
    sha = "e81a72f97f592910e909a2352d6b8c0de0801c51ac1383bad4ebf3f2ecdd2fd8", file = "alass-windows64.zip", zip = true,
    exe = "alass-windows64\\bin\\alass-cli.exe" },
}

local function toast(msg, secs)
  mp.osd_message(msg, secs or 4)
  mp.commandv("script-message", "vivi-toast", msg)
end

local running = false
local found -- cache: { alass = path, ffsubsync = path, managed = bool }

local function managed_alass(plat)
  local a = ALASS[plat]
  if not a then return nil end
  local p = ALASS_DIR .. SEP .. (a.exe or a.file)
  return utils.file_info(p) and p or nil
end

-- System tools (and an already downloaded alass). cb(found)
local function detect(cb)
  if found then return cb(found) end
  local f = {}
  find_program({ "alass", "alass-cli" }, { "--version" }, function(a)
    f.alass = a
    find_program({ "ffsubsync" }, { "--version" }, function(s)
      f.ffsubsync = s
      platform(function(plat)
        if not f.alass then
          f.alass = managed_alass(plat)
          f.managed = f.alass ~= nil
        end
        f.plat = plat
        found = f
        cb(f)
      end)
    end)
  end)
end

local MISSING = "Subtitle sync needs alass or ffsubsync: cargo install alass-cli, or pip install ffsubsync"

-- Tool to use, downloading alass if allowed. cb(tool, path | nil, err)
local function pick(cb)
  detect(function(f)
    local want = (o.tool or "Auto"):lower()
    if want == "ffsubsync" then
      if f.ffsubsync then return cb("ffsubsync", f.ffsubsync) end
      return cb(nil, nil, "ffsubsync is not installed; set Sync tool to Auto")
    end
    if f.alass then return cb("alass", f.alass) end
    if want == "auto" and f.ffsubsync then return cb("ffsubsync", f.ffsubsync) end
    local a = ALASS[f.plat]
    if not o.download or not a then return cb(nil, nil, MISSING) end
    fetch_verified("alass", a.url, a.sha, ALASS_DIR, a.file, a.zip, function(ok, err)
      if not ok then return cb(nil, nil, err) end
      found = nil
      local p = managed_alass(f.plat)
      if not p then return cb(nil, nil, "alass missing from the download") end
      cb("alass", p)
    end)
  end)
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

local function run_sync(tool, exe, video, sub, out, env)
  local args
  if tool == "alass" then
    args = { exe, video, sub, out }
  else
    args = { exe, video, "-i", sub, "-o", out }
  end
  if env and not windows then
    local e = { "env" }
    for k, v in pairs(env) do e[#e + 1] = k .. "=" .. v end
    for _, x in ipairs(args) do e[#e + 1] = x end
    args = e
  end
  running = true
  toast("Syncing subtitle with " .. tool .. "…", 3)
  mp.msg.info("running: " .. table.concat(args, " "))
  local t0 = mp.get_time()
  run(args, function(r)
    running = false
    if r.status ~= 0 or not utils.file_info(out) then
      local err = (r.stderr and r.stderr ~= "" and r.stderr) or r.stdout or r.error_string or ""
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

local function sync()
  if running then toast("Subtitle sync is already running"); return end
  local video = mp.get_property("path")
  if not video or video:find("://") then toast("Subtitle sync works on local files only"); return end
  local sub = current_sub()
  if not sub then toast("Select an external subtitle first (embedded tracks are not supported)"); return end
  running = true
  pick(function(tool, exe, err)
    running = false
    if not tool then return toast(err or MISSING, 8) end
    local out = out_path(sub)
    if tool ~= "alass" or windows then return run_sync(tool, exe, video, sub, out) end
    -- alass needs ffmpeg + ffprobe: system first, else the shared media build.
    running = true
    get_ffmpeg(true, function(ff, e)
      running = false
      if not ff then return toast("alass needs ffmpeg: " .. tostring(e), 8) end
      run_sync(tool, exe, video, sub, out, ff.kind == "managed"
        and { ALASS_FFMPEG_PATH = ff.ffmpeg, ALASS_FFPROBE_PATH = ff.ffprobe } or nil)
    end)
  end)
end

local function report()
  found, ff_cache = nil, nil
  detect(function(f)
    local list = {}
    if f.alass then list[#list + 1] = (f.managed and "alass (downloaded): " or "alass: ") .. f.alass end
    if f.ffsubsync then list[#list + 1] = "ffsubsync: " .. f.ffsubsync end
    get_ffmpeg(false, function(ff)
      list[#list + 1] = "ffmpeg: " .. (ff and (ff.ffmpeg .. " (" .. ff.kind .. ")") or "none (downloaded when alass needs it)")
      if not f.alass and not f.ffsubsync then
        list[#list + 1] = ALASS[f.plat] and (o.download and "alass is downloaded on the first sync" or "downloads are off")
          or MISSING
      end
      toast(table.concat(list, "\n"), 8)
    end)
  end)
end

-- ---------- manual offset, remembered per file ----------

local function read_offsets()
  local f = io.open(OFFSETS, "r")
  if not f then return {} end
  local v = utils.parse_json(f:read("*a") or "")
  f:close()
  return type(v) == "table" and v or {}
end

local function write_offsets(t)
  mkdir(DIR, function()
    local f = io.open(OFFSETS .. ".tmp", "w")
    if not f then return end
    f:write(utils.format_json(t))
    f:close()
    os.remove(OFFSETS)
    os.rename(OFFSETS .. ".tmp", OFFSETS)
  end)
end

-- Key of a file: path + size (survives renaming the subtitle, not moving the video).
local function file_key(path)
  if not path or path == "" then return nil end
  local fi = utils.file_info(path)
  return path .. "|" .. tostring(fi and fi.size or 0)
end

local function round(x) return math.floor(x * 1000 + 0.5) / 1000 end

local function set_offset(v, quiet)
  v = round(v)
  mp.set_property_number("sub-delay", v)
  local k = file_key(mp.get_property("path"))
  if k then
    local t = read_offsets()
    t[k] = v ~= 0 and v or nil
    write_offsets(t)
  end
  if not quiet then
    mp.osd_message(string.format("Subtitle offset %+.1f s (saved for this file)", v), 2)
  end
end

local function nudge(d)
  return function() set_offset((mp.get_property_number("sub-delay") or 0) + d * (tonumber(o.step) or 0.1)) end
end

local function reset()
  set_offset(0, true)
  mp.osd_message("Subtitle offset reset", 2)
end

mp.register_event("file-loaded", function()
  local k = file_key(mp.get_property("path"))
  local v = k and read_offsets()[k]
  -- After Vivi's per-show preset (applied on file-loaded too), so the file's own value wins.
  if v then mp.add_timeout(0.2, function()
    mp.set_property_number("sub-delay", v)
    mp.msg.info(string.format("subtitle offset %+.3f s restored", v))
  end) end
end)

mp.add_forced_key_binding("Alt+y", "sync", sync)
mp.add_forced_key_binding("Alt+z", "earlier", nudge(-1), { repeatable = true })
mp.add_forced_key_binding("Alt+x", "later", nudge(1), { repeatable = true })
mp.add_forced_key_binding("Alt+Z", "earlier-1s", nudge(-10), { repeatable = true })
mp.add_forced_key_binding("Alt+X", "later-1s", nudge(10), { repeatable = true })
mp.add_forced_key_binding("Alt+BS", "reset", reset)
mp.register_script_message("sync", sync)
mp.register_script_message("detect", report)
mp.register_script_message("reset", reset)

if VIVI_TEST then
  return { out_path = out_path, file_key = file_key, round = round, set_offset = set_offset,
    read_offsets = read_offsets, nudge = nudge, OFFSETS = OFFSETS }
end
