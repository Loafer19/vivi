-- @name AcoustID Tagger
-- @version 1.0.0
-- @author Vivi
-- @description Identify songs by their sound (Chromaprint fpcalc + AcoustID) and review tags and a new file name in the Auto-tagger (Vivi plugin package).
--
-- Keys: Alt+i  identify the playing file
-- Script messages: identify, detect.
--
-- 1. fpcalc (Chromaprint) fingerprints the first two minutes: a system fpcalc first
--    (package chromaprint / libchromaprint-tools, brew chromaprint), otherwise the
--    official static build of Chromaprint 1.6.2 is downloaded once from
--    https://github.com/acoustid/chromaprint/releases (1.3-2.6 MB, sha256 pinned below)
--    into plugin-data/acoustid/.
-- 2. POST https://api.acoustid.org/v2/lookup (client = your application API key,
--    meta = recordings) with curl.
-- 3. The answer goes to Vivi's Auto-tagger (`script-message vivi-tagger-acoustid <path>
--    <json>`), which looks the recordings up on MusicBrainz and opens its candidates list
--    and before/after review, including the file name built from `pattern`.
--
-- AcoustID requires an application key and Vivi does not ship one: register a free
-- application at https://acoustid.org/new-application and paste its key in the settings
-- (same as the OpenSubtitles key of Subtitle Search).
local mp = require "mp"
local options = require "mp.options"
local utils = require "mp.utils"

local o = { ["api-key"] = "", pattern = "{artist} - {title}", download = true }
options.read_options(o, "acoustid")

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
-- ---------- end of managed helpers ----------

local DIR = PLUGIN_DATA .. SEP .. "acoustid"
local V = "1.6.2"
local FPCALC = {
  ["linux-x86_64"] = { "linux-x86_64", "tar.gz", "c968ad4a0821592a79b158fb2e1f46cbe0a22a0a1dbc0743c67d68e75d15d2bb" },
  ["linux-aarch64"] = { "linux-arm64", "tar.gz", "3aa1498126ed272d805bc9d081c77965c9e1e1a0891bb112c13b56ef2ad47bad" },
  ["macos-aarch64"] = { "macos-universal", "tar.gz", "53d9368f96017fb288d74f6ed4d928f7bbbfb2ff81e69b4ef67b6ceeb217d9b2" },
  ["macos-x86_64"] = { "macos-universal", "tar.gz", "53d9368f96017fb288d74f6ed4d928f7bbbfb2ff81e69b4ef67b6ceeb217d9b2" },
  ["windows-x86_64"] = { "windows-x86_64", "zip", "b74369726676670af3ab0eadff45f960cf969fdf4a87a971ba37eaf8d530e874" },
}

local function toast(msg, secs)
  mp.osd_message(msg, secs or 4)
  mp.commandv("script-message", "vivi-toast", msg)
end

local function managed_path(plat)
  local f = FPCALC[plat]
  if not f then return nil end
  return DIR .. SEP .. "chromaprint-fpcalc-" .. V .. "-" .. f[1] .. SEP .. "fpcalc" .. EXE
end

local fp_cache
-- cb(path | nil, err, kind)
local function get_fpcalc(cb)
  if fp_cache then return cb(fp_cache.path, nil, fp_cache.kind) end
  find_program({ "fpcalc" }, { "-version" }, function(p)
    if p then fp_cache = { path = p, kind = "system" } return cb(p, nil, "system") end
    platform(function(plat)
      local m = managed_path(plat)
      if m and utils.file_info(m) then fp_cache = { path = m, kind = "downloaded" } return cb(m, nil, "downloaded") end
      local f = FPCALC[plat]
      if not f then return cb(nil, "no fpcalc build for " .. plat .. "; install chromaprint") end
      if not o.download then return cb(nil, "fpcalc is not installed (install chromaprint, or allow the download)") end
      local name = "chromaprint-fpcalc-" .. V .. "-" .. f[1] .. "." .. f[2]
      fetch_verified("fpcalc", "https://github.com/acoustid/chromaprint/releases/download/v" .. V .. "/" .. name,
        f[3], DIR, name, true, function(ok, err)
          if not ok then return cb(nil, err) end
          if not utils.file_info(m) then return cb(nil, "fpcalc missing from the download") end
          fp_cache = { path = m, kind = "downloaded" }
          cb(m, nil, "downloaded")
        end)
    end)
  end)
end

-- { duration, fingerprint } from `fpcalc -json` output, or nil.
local function parse_fpcalc(out)
  local v = utils.parse_json(out or "")
  if type(v) ~= "table" or not v.fingerprint or not v.duration then return nil end
  return { duration = math.floor(tonumber(v.duration) + 0.5), fingerprint = v.fingerprint }
end

local function urlencode(s)
  return (tostring(s):gsub("[^%w%-_%.~]", function(c) return string.format("%%%02X", c:byte()) end))
end

local function lookup_body(key, fp)
  return "client=" .. urlencode(key) .. "&meta=recordings&duration=" .. fp.duration ..
    "&fingerprint=" .. urlencode(fp.fingerprint)
end

-- "Artist - Title" of the best recording in an AcoustID answer, and the result count.
local function best(v)
  local top, n = nil, 0
  for _, r in ipairs(v.results or {}) do
    for _, rec in ipairs(r.recordings or {}) do
      n = n + 1
      if not top and rec.title then
        local a = {}
        for _, x in ipairs(rec.artists or {}) do a[#a + 1] = x.name end
        top = string.format("%s - %s (%d%%)", table.concat(a, ", "), rec.title, math.floor((r.score or 0) * 100 + 0.5))
      end
    end
  end
  return top, n
end

local busy = false

local function identify()
  if busy then return toast("AcoustID: already identifying") end
  local path = mp.get_property("path")
  if not path or path:find("://") or not utils.file_info(path) then return toast("AcoustID: play a local audio file first") end
  local key = (o["api-key"] or ""):gsub("%s", "")
  if key == "" then
    return toast("AcoustID needs your free application key: acoustid.org/new-application → Settings → Extensions → AcoustID", 8)
  end
  busy = true
  get_fpcalc(function(fpcalc, err)
    if not fpcalc then busy = false return toast("AcoustID: " .. err, 8) end
    mp.osd_message("Fingerprinting…", 3)
    run({ fpcalc, "-json", path }, function(r)
      local fp = r.status == 0 and parse_fpcalc(r.stdout) or nil
      if not fp then busy = false return toast("fpcalc could not read this file: " .. ((r.stderr or ""):match("[^\n]+") or "")) end
      run({ "curl", "-sS", "-m", "20", "-A", "Vivi-acoustid/1.0.0", "--data-binary", "@-",
            "-H", "Content-Type: application/x-www-form-urlencoded", "https://api.acoustid.org/v2/lookup" },
        function(res)
          busy = false
          local v = res.status == 0 and utils.parse_json(res.stdout or "") or nil
          if type(v) ~= "table" then return toast("AcoustID: no answer (" .. ((res.stderr or ""):match("[^\n]+") or "offline?") .. ")") end
          if v.status ~= "ok" then
            return toast("AcoustID: " .. tostring(v.error and v.error.message or "lookup failed"), 6)
          end
          local top, n = best(v)
          if n == 0 then return toast("AcoustID: this recording is not in the database yet") end
          mp.osd_message("AcoustID: " .. (top or (n .. " matches")) .. " — opening the review", 4)
          mp.commandv("script-message", "vivi-tagger-acoustid", path,
            utils.format_json({ acoustid = v, pattern = o.pattern or "" }))
        end, { stdin_data = lookup_body(key, fp) })
    end)
  end)
end

local function detect()
  fp_cache = nil
  get_fpcalc(function(p, err, kind)
    toast(p and ("fpcalc (" .. kind .. "): " .. p) or ("fpcalc: " .. err), 6)
  end)
end

mp.add_forced_key_binding("Alt+i", "identify", identify)
mp.register_script_message("identify", identify)
mp.register_script_message("detect", detect)

if VIVI_TEST then
  return { parse_fpcalc = parse_fpcalc, lookup_body = lookup_body, best = best, managed_path = managed_path }
end
