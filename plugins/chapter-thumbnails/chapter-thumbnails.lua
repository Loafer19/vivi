-- @name Chapter Thumbnails
-- @version 1.0.0
-- @author Vivi
-- @description Seek-bar hover thumbnails and chapter thumbnails, cached per file (Vivi plugin package).
--
-- Keys: Alt+h  chapter list (titles, times; thumbnails are in the side panel's Chapters tab)
-- Script messages: generate, list, clear.
--
-- For every local video it writes, with ffmpeg (fast keyframe seeks):
--   plugin-data/chapter-thumbnails/<key>/seek-00.jpg … seek-49.jpg  192×108, evenly spaced
--   plugin-data/chapter-thumbnails/<key>/chapter-<n>.jpg            320 px wide, n from 1
-- where <key> is the hash of "path|size|mtime" used by Vivi (src/scenes.rs). Vivi shows
-- the seek frames when hovering the seek bar and the chapter images in the Chapters tab
-- (`script-message vivi-preview-frames` / `vivi-chapter-thumbs` tell it to reload). Cached
-- folders are reused when the file is opened again; the oldest are pruned (max-cache).
--
-- ffmpeg: a system ffmpeg first; otherwise Vivi's media ffmpeg build (about 15 MB with
-- ffprobe, downloaded once and shared with Subtitle Auto-Sync).
local mp = require "mp"
local options = require "mp.options"
local utils = require "mp.utils"

local o = { seekbar = true, chapters = true, frames = "50", ["max-cache"] = "200", download = true }

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

local ROOT = PLUGIN_DATA .. SEP .. "chapter-thumbnails"
local SEEK_W, SEEK_H = 192, 108

local function toast(msg, secs)
  mp.osd_message(msg, secs or 3)
  mp.commandv("script-message", "vivi-toast", msg)
end

-- Same key as Vivi's scenes::key: 32-bit "h*33 + byte" hash of path|size|mtime, hex.
local function key(path)
  local fi = utils.file_info(path)
  if not fi then return nil end
  local s = path .. "|" .. tostring(fi.size) .. "|" .. tostring(math.floor(fi.mtime or 0))
  local h = 5381
  for i = 1, #s do h = (h * 33 + s:byte(i)) % 4294967296 end
  return string.format("%08x", h)
end

-- Times (s) of the seek-bar frames: centre of each of `n` equal slices.
local function seek_times(duration, n)
  local t = {}
  for i = 0, n - 1 do t[#t + 1] = duration * (i + 0.5) / n end
  return t
end

local function frame_args(ff, path, t, out, w, h)
  local vf = h and string.format("scale=%d:%d:force_original_aspect_ratio=decrease,pad=%d:%d:(ow-iw)/2:(oh-ih)/2", w, h, w, h)
    or string.format("scale=%d:-2", w)
  return { ff, "-nostdin", "-v", "error", "-y", "-skip_frame", "nokey", "-ss", string.format("%.2f", t),
    "-i", path, "-an", "-sn", "-dn", "-frames:v", "1", "-vf", vf, "-q:v", "5", out }
end

local gen = 0       -- bumped on every file so stale jobs stop
local busy = false
local seen_chapters = -1 -- chapter count the last run covered

local function local_video()
  local path = mp.get_property("path")
  if not path or path:find("://") or not utils.file_info(path) then return nil end
  for _, t in ipairs(mp.get_property_native("track-list") or {}) do
    if t.type == "video" and not t.albumart and not t.image then return path end
  end
  return nil
end

-- Run frame jobs, two at a time; cb() when all are done. A job whose keyframe-only
-- seek produced nothing (past the last keyframe) is retried once with normal decoding.
local function jobs(list, my, cb)
  local i, left = 0, #list
  if left == 0 then return cb() end
  local function nxt()
    if my ~= gen then return end
    i = i + 1
    local j = list[i]
    if not j then return end
    local function go(args, retry)
      run(args, function()
        if retry and j.out and not utils.file_info(j.out) then
          local a = {}
          for _, x in ipairs(args) do if x ~= "-skip_frame" and x ~= "nokey" then a[#a + 1] = x end end
          return go(a, false)
        end
        left = left - 1
        if j.done then j.done() end
        if left == 0 then return cb() end
        nxt()
      end)
    end
    go(j.args, true)
  end
  nxt()
  nxt()
end

-- Copy a file (binary).
local function copy(a, b)
  local f = io.open(a, "rb")
  if not f then return false end
  local d = f:read("*a")
  f:close()
  local g = io.open(b, "wb")
  if not g then return false end
  g:write(d)
  g:close()
  return true
end

-- Every seek frame must exist (Vivi maps the hover position onto the frame count):
-- a missing one borrows its nearest earlier (or later) neighbour.
local function fill_gaps(dir, n)
  local name = function(k) return string.format("%s%sseek-%02d.jpg", dir, SEP, k) end
  for k = 0, n - 1 do
    if not utils.file_info(name(k)) then
      for d = 1, n do
        local src = (k - d >= 0 and utils.file_info(name(k - d)) and name(k - d))
          or (k + d < n and utils.file_info(name(k + d)) and name(k + d))
        if src then copy(src, name(k)) break end
      end
    end
  end
end

-- Remove the least recently written cache folders beyond max-cache.
local function prune()
  local max = tonumber(o["max-cache"]) or 200
  local dirs = utils.readdir(ROOT, "dirs") or {}
  if #dirs <= max then return end
  local list = {}
  for _, d in ipairs(dirs) do
    local fi = utils.file_info(ROOT .. SEP .. d)
    list[#list + 1] = { d, fi and fi.mtime or 0 }
  end
  table.sort(list, function(a, b) return a[2] < b[2] end)
  for k = 1, #list - max do
    local p = ROOT .. SEP .. list[k][1]
    if windows then run({ "cmd", "/c", "rmdir", "/s", "/q", p }, function() end)
    else run({ "rm", "-rf", p }, function() end) end
  end
end

local function generate(manual)
  gen = gen + 1
  local my = gen
  local path = local_video()
  if not path then if manual then toast("Thumbnails: play a local video first") end return end
  local duration = mp.get_property_number("duration", 0)
  if duration <= 1 then return end
  local k = key(path)
  local dir = ROOT .. SEP .. k
  seen_chapters = #(mp.get_property_native("chapter-list") or {})
  local n = math.max(10, math.min(200, tonumber(o.frames) or 50))
  local seek, chap = {}, {}
  if o.seekbar then
    for i, t in ipairs(seek_times(duration, n)) do
      local out = string.format("%s%sseek-%02d.jpg", dir, SEP, i - 1)
      if not utils.file_info(out) then seek[#seek + 1] = { t = t, out = out } end
    end
  end
  if o.chapters then
    for i, c in ipairs(mp.get_property_native("chapter-list") or {}) do
      local out = string.format("%s%schapter-%d.jpg", dir, SEP, i)
      if not utils.file_info(out) then
        chap[#chap + 1] = { t = math.min(c.time + 1, math.max(0, duration - 1)), out = out }
      end
    end
  end
  if #seek == 0 and #chap == 0 then
    if o.seekbar then mp.commandv("script-message", "vivi-preview-frames", dir) end
    if o.chapters then mp.commandv("script-message", "vivi-chapter-thumbs", dir) end
    if manual then toast("Thumbnails are up to date") end
    return
  end
  busy = true
  get_ffmpeg(o.download, function(ff, err)
    if my ~= gen then busy = false return end
    if not ff then
      busy = false
      if manual or o.download then toast("Thumbnails need ffmpeg: " .. tostring(err), 6) end
      return
    end
    mkdir(dir, function()
      local t0 = mp.get_time()
      local list = {}
      -- Chapters first (few), then the seek-bar frames.
      for _, c in ipairs(chap) do list[#list + 1] = { out = c.out, args = frame_args(ff.ffmpeg, path, c.t, c.out, 320) } end
      local nchap = #list
      for _, s in ipairs(seek) do list[#list + 1] = { out = s.out, args = frame_args(ff.ffmpeg, path, s.t, s.out, SEEK_W, SEEK_H) } end
      if nchap > 0 then
        list[nchap].done = function() mp.commandv("script-message", "vivi-chapter-thumbs", dir) end
      end
      jobs(list, my, function()
        busy = false
        if #seek > 0 then fill_gaps(dir, n) end
        if #seek > 0 then mp.commandv("script-message", "vivi-preview-frames", dir) end
        mp.msg.info(string.format("%d thumbnails in %.1f s (%s ffmpeg)", #list, mp.get_time() - t0, ff.kind))
        if manual then toast(string.format("%d thumbnails generated", #list)) end
        prune()
      end)
    end)
  end)
end

local function fmt_time(t)
  t = math.floor(t or 0)
  local h, m, s = math.floor(t / 3600), math.floor(t / 60) % 60, t % 60
  if h > 0 then return string.format("%d:%02d:%02d", h, m, s) end
  return string.format("%d:%02d", m, s)
end

local function chapter_list()
  local list = mp.get_property_native("chapter-list") or {}
  if #list == 0 then toast("This file has no chapters (Scene Chapters can detect scenes)") return end
  local cur = mp.get_property_number("chapter", -1) + 1
  local lines = {}
  for i, c in ipairs(list) do
    lines[#lines + 1] = string.format("%s%s  %s", i == cur and "▶ " or "   ", fmt_time(c.time),
      (c.title and c.title ~= "") and c.title or ("Chapter " .. i))
  end
  mp.osd_message(table.concat(lines, "\n"), 6)
end

local function clear()
  local path = local_video()
  if not path then return end
  local dir = ROOT .. SEP .. key(path)
  if windows then run({ "cmd", "/c", "rmdir", "/s", "/q", dir }, function() toast("Thumbnails cleared") end)
  else run({ "rm", "-rf", dir }, function() toast("Thumbnails cleared") end) end
end

options.read_options(o, "chapter-thumbnails", function() generate(false) end)
mp.register_event("file-loaded", function() generate(false) end)
mp.register_event("start-file", function() gen = gen + 1 seen_chapters = -1 end)
-- Chapters can arrive later (Scene Chapters, SponsorBlock): make their images too.
mp.observe_property("chapter-list", "native", function(_, v)
  if seen_chapters >= 0 and not busy and #(v or {}) ~= seen_chapters then generate(false) end
end)
mp.add_forced_key_binding("Alt+h", "list", chapter_list)
mp.register_script_message("generate", function() generate(true) end)
mp.register_script_message("list", chapter_list)
mp.register_script_message("clear", clear)

if VIVI_TEST then
  return { key = key, seek_times = seek_times, frame_args = frame_args, fmt_time = fmt_time }
end
