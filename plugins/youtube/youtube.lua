-- @name YouTube & yt-dlp
-- @version 1.1.2
-- @author Vivi
-- @description List, size and download YouTube (and other yt-dlp) videos and playlists.
--
-- Keys: Alt+u  paste a URL from the clipboard and add its entries to the playlist
--
-- yt-dlp: a system copy is preferred when it is recent (released in the last 90 days):
-- PATH and common install folders. Otherwise the plugin downloads its own copy from the official GitHub releases
-- (https://github.com/yt-dlp/yt-dlp/releases/latest), checks it against SHA2-256SUMS and
-- keeps it in $XDG_DATA_HOME/vivi/plugin-data/youtube/bin, where Vivi also looks for it
-- (it appends that folder to PATH, so mpv's ytdl_hook finds it in the Flatpak). Only that
-- managed copy is updated (`yt-dlp -U`, daily when "auto-update" is on). The chosen yt-dlp
-- is also handed to mpv's ytdl_hook (script-opt ytdl_hook-ytdl_path) for streaming.
--
-- ffmpeg (merging separate video + audio, audio conversion): a system ffmpeg is used when
-- there is one (not inside the Flatpak, whose runtime has no ffmpeg program). A managed
-- static build is downloaded only when needed: video at 1080p or more / "Best", or
-- audio converted to mp3/m4a/opus. Sources: BtbN/FFmpeg-Builds (Linux x86_64/aarch64,
-- Windows x64/arm64; archive checked against checksums.sha256) and osxexperts.net
-- (macOS arm64/Intel; binaries checked against the sha256 on that page). Kept in
-- plugin-data/youtube/ffmpeg (ffmpeg + ffprobe), passed to yt-dlp with
-- --ffmpeg-location; only this copy is updated (*Update ffmpeg*, or every 30 days with
-- auto-update when the published build changed). Lower qualities without ffmpeg use
-- single-file formats.
--
-- Sizes: from yt-dlp's filesize / filesize_approx (or bitrate × duration) of the formats
-- the chosen quality would pick; shown before every download and with "Show sizes".
-- Progress: yt-dlp writes a progress template line to a log file that is read every
-- 2 s (Linux/macOS; on Windows only start and end are reported).
local mp = require "mp"
local options = require "mp.options"
local utils = require "mp.utils"

local o = {
  url = "",
  quality = "1080p",
  mode = "Video",
  ["audio-format"] = "Original",
  folder = "",
  source = "Auto (system if recent, else managed)",
  ["auto-update"] = true,
  pkgdir = "", -- set by Vivi
}

local windows = package.config:sub(1, 1) == "\\"
local SEP = windows and "\\" or "/"
local EXE = windows and ".exe" or ""
local HOME = os.getenv("HOME") or os.getenv("USERPROFILE") or "."
local DATA_HOME = os.getenv("XDG_DATA_HOME")
if not DATA_HOME or DATA_HOME == "" then DATA_HOME = HOME .. SEP .. ".local" .. SEP .. "share" end
local DATA = DATA_HOME .. SEP .. "vivi" .. SEP .. "plugin-data" .. SEP .. "youtube"
local BIN_DIR = DATA .. SEP .. "bin"
local MANAGED = BIN_DIR .. SEP .. "yt-dlp" .. EXE
local RELEASES = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/"
local MAX_AGE = 90 * 86400
local FLATPAK = utils.file_info("/.flatpak-info") ~= nil

local ytdlp = nil      -- { cmd = {...}, kind = "system"|"managed", version, path }
local resolving = nil  -- callbacks waiting for detection
local ffmpeg = nil     -- true/false once checked
local job = nil        -- running download
local info_cache = {}  -- url -> yt-dlp JSON (single video)

local function toast(msg, secs)
  mp.msg.info(msg)
  mp.osd_message(msg, secs or 3)
  mp.commandv("script-message", "vivi-toast", msg)
end

local function run(args, cb, capture)
  return mp.command_native_async({
    name = "subprocess", args = args, playback_only = false,
    capture_stdout = capture ~= false, capture_stderr = true,
  }, function(_, r) cb(r or {}) end)
end

local function concat(a, b)
  local t = {}
  for _, v in ipairs(a) do t[#t + 1] = v end
  for _, v in ipairs(b) do t[#t + 1] = v end
  return t
end

local function mkdir(dir, cb)
  if windows then
    run({ "cmd", "/c", "if not exist \"" .. dir .. "\" mkdir \"" .. dir .. "\"" }, cb)
  else
    run({ "mkdir", "-p", dir }, cb)
  end
end

local function human(n)
  if not n then return "?" end
  if n >= 1073741824 then return string.format("%.1f GB", n / 1073741824) end
  if n >= 1048576 then return string.format("%.0f MB", n / 1048576) end
  return string.format("%.0f KB", n / 1024)
end

-- ---------- finding yt-dlp ----------

-- "2026.08.19" -> unix time (nil when not a yt-dlp date version).
local function version_time(v)
  local y, m, d = (v or ""):match("(%d%d%d%d)%.(%d%d?)%.(%d%d?)")
  if not y then return nil end
  return os.time({ year = tonumber(y), month = tonumber(m), day = tonumber(d), hour = 12 })
end

local function recent(v)
  local t = version_time(v)
  return t ~= nil and os.time() - t <= MAX_AGE
end

-- Executables named yt-dlp outside our own folder: PATH first, then common places.
local function system_candidates()
  local seen, out = {}, {}
  local function add(dir)
    if not dir or dir == "" or dir == BIN_DIR then return end
    local p = dir .. SEP .. "yt-dlp" .. EXE
    if not seen[p] and utils.file_info(p) then
      seen[p] = true
      out[#out + 1] = p
    end
  end
  for dir in (os.getenv("PATH") or ""):gmatch(windows and "[^;]+" or "[^:]+") do add(dir) end
  if windows then
    local la = os.getenv("LOCALAPPDATA") or ""
    add(la .. "\\Microsoft\\WinGet\\Links")
    add(HOME .. "\\scoop\\shims")
    add("C:\\ProgramData\\chocolatey\\bin")
  else
    for _, d in ipairs({ HOME .. "/.local/bin", HOME .. "/bin", "/usr/local/bin", "/usr/bin",
                         "/opt/homebrew/bin", "/opt/local/bin", "/snap/bin" }) do add(d) end
  end
  return out
end

local function probe(cmd, cb)
  run(concat(cmd, { "--version" }), function(r)
    local v = r.status == 0 and (r.stdout or ""):match("^%s*(%S+)") or nil
    cb(v)
  end)
end

-- First working system yt-dlp: { cmd, version, path, kind }.
local function find_system(cb)
  local list = system_candidates()
  local i = 0
  local function nxt()
    i = i + 1
    if list[i] then
      probe({ list[i] }, function(v)
        if v then cb({ cmd = { list[i] }, version = v, path = list[i], kind = "system" }) else nxt() end
      end)
    else
      cb(nil)
    end
  end
  nxt()
end

-- Release asset for this computer.
local function asset_name(cb)
  if windows then
    local a = (os.getenv("PROCESSOR_ARCHITEW6432") or os.getenv("PROCESSOR_ARCHITECTURE") or ""):upper()
    cb(a == "ARM64" and "yt-dlp_arm64.exe" or a == "X86" and "yt-dlp_x86.exe" or "yt-dlp.exe")
    return
  end
  run({ "uname", "-sm" }, function(r)
    local s = r.stdout or ""
    if s:match("Darwin") then cb("yt-dlp_macos")
    elseif s:match("x86_64") then cb("yt-dlp_linux")
    elseif s:match("aarch64") or s:match("arm64") then cb("yt-dlp_linux_aarch64")
    else cb(nil, "no yt-dlp build for " .. s:gsub("%s+$", "")) end
  end)
end

local function sha256(file, cb)
  local function first_hex(s)
    for line in (s or ""):gmatch("[^\r\n]+") do
      local h = line:gsub("%s", ""):lower():match("^(%x+)")
      if h and #h >= 64 then return h:sub(1, 64) end
    end
  end
  if windows then
    run({ "certutil", "-hashfile", file, "SHA256" }, function(r)
      local out = (r.stdout or ""):gsub("^[^\n]*\n", "") -- skip the "SHA256 hash of" line
      cb(first_hex(out))
    end)
    return
  end
  run({ "sha256sum", file }, function(r)
    if r.status == 0 then return cb(first_hex(r.stdout)) end
    run({ "shasum", "-a", "256", file }, function(r2) cb(r2.status == 0 and first_hex(r2.stdout) or nil) end)
  end)
end

-- Download the managed copy (verified against the release's SHA2-256SUMS).
local function install_managed(cb)
  asset_name(function(asset, err)
    if not asset then return cb(nil, err) end
    mkdir(BIN_DIR, function()
      local tmp = MANAGED .. ".part"
      toast("Downloading yt-dlp (" .. asset .. ")…", 5)
      run({ "curl", "-fsSL", "--retry", "2", "-o", tmp, RELEASES .. asset }, function(r)
        if r.status ~= 0 then return cb(nil, "yt-dlp download failed: " .. (r.stderr or r.error_string or "")) end
        run({ "curl", "-fsSL", RELEASES .. "SHA2-256SUMS" }, function(s)
          local want
          for h, n in (s.stdout or ""):gmatch("(%x+)%s+%*?([^\r\n]+)") do
            if n == asset then want = h:lower() end
          end
          sha256(tmp, function(got)
            if not want or not got or got ~= want then
              os.remove(tmp)
              return cb(nil, not got and "no sha256 tool (sha256sum/shasum/certutil) to verify yt-dlp"
                or "yt-dlp checksum mismatch, not installed")
            end
            os.remove(MANAGED)
            local ok, e = os.rename(tmp, MANAGED)
            if not ok then return cb(nil, e) end
            local function done()
              probe({ MANAGED }, function(v)
                if not v then return cb(nil, "the downloaded yt-dlp does not run") end
                local f = io.open(DATA .. SEP .. "last-update", "w")
                if f then f:write(tostring(os.time())) f:close() end
                cb({ cmd = { MANAGED }, version = v, path = MANAGED, kind = "managed" })
              end)
            end
            if windows then done() else run({ "chmod", "755", MANAGED }, done) end
          end)
        end)
      end)
    end)
  end)
end

local function self_update(quiet, cb)
  if not utils.file_info(MANAGED) then
    if not quiet then toast("The managed yt-dlp is not installed (a system yt-dlp is used)") end
    return cb and cb()
  end
  run({ MANAGED, "-U" }, function(r)
    local f = io.open(DATA .. SEP .. "last-update", "w")
    if f then f:write(tostring(os.time())) f:close() end
    local out = ((r.stdout or "") .. (r.stderr or "")):gsub("%s+$", "")
    local last = out:match("([^\n]+)$") or "done"
    if ytdlp and ytdlp.kind == "managed" then
      probe({ MANAGED }, function(v) ytdlp.version = v or ytdlp.version end)
    end
    if not quiet or out:match("Updated yt%-dlp") then toast("yt-dlp: " .. last, 5) end
    if cb then cb() end
  end)
end

local function maybe_auto_update()
  if not o["auto-update"] then return end
  local f = io.open(DATA .. SEP .. "last-update", "r")
  local t = f and tonumber(f:read("*a")) or 0
  if f then f:close() end
  if os.time() - t > 86400 then self_update(true) end
end

-- Hand the chosen yt-dlp to mpv's ytdl_hook.
local function tell_ytdl_hook(y)
  mp.commandv("change-list", "script-opts", "append", "ytdl_hook-ytdl_path=" .. y.path)
end

local function chose(y, cb_err)
  ytdlp = y
  if y then
    mp.msg.info(string.format("using %s yt-dlp %s (%s)", y.kind, y.version, y.path))
    tell_ytdl_hook(y)
    if y.kind == "managed" then maybe_auto_update() end
  end
  local waiting = resolving or {}
  resolving = nil
  for _, cb in ipairs(waiting) do cb(y, cb_err) end
end

-- Resolve the yt-dlp to use per the "source" setting, then cb(ytdlp | nil, err).
local function ensure(cb)
  if ytdlp then return cb(ytdlp) end
  if resolving then resolving[#resolving + 1] = cb return end
  resolving = { cb }
  local src = o.source
  local function managed(fallback)
    if utils.file_info(MANAGED) then
      probe({ MANAGED }, function(v)
        if v then return chose({ cmd = { MANAGED }, version = v, path = MANAGED, kind = "managed" }) end
        install_managed(function(y, e) chose(y or fallback, not (y or fallback) and e or nil) end)
      end)
    else
      install_managed(function(y, e)
        if not y and e then toast(e, 6) end
        chose(y or fallback, not (y or fallback) and e or nil)
      end)
    end
  end
  if src == "Managed copy" then return managed(nil) end
  find_system(function(sys)
    if src == "System" then
      if not sys then return chose(nil, "no system yt-dlp found; choose \"Managed copy\" in the plugin settings") end
      if not recent(sys.version) then toast("System yt-dlp " .. sys.version .. " is old; YouTube may fail", 5) end
      return chose(sys)
    end
    if sys and recent(sys.version) then return chose(sys) end
    managed(sys) -- old system copy only as a fallback
  end)
end

local function with_ytdlp(cb)
  ensure(function(y, err)
    if not y then return toast(err or "yt-dlp is not available", 6) end
    cb(y)
  end)
end

local function ytdlp_json(y, args, cb)
  run(concat(y.cmd, concat({ "-J", "--no-warnings" }, args)), function(r)
    if r.status ~= 0 then
      local e = (r.stderr or ""):match("ERROR: ([^\n]+)") or r.error_string or "yt-dlp failed"
      return cb(nil, e)
    end
    local j = utils.parse_json(r.stdout or "")
    if not j then return cb(nil, "yt-dlp returned no data") end
    cb(j)
  end)
end

-- ---------- quality, sizes, formats ----------

local function height_limit(q)
  return tonumber((q or ""):match("^(%d+)p")) or math.huge
end

local function fsize(f, dur)
  local s = f.filesize or f.filesize_approx
  if not s and f.tbr and dur then s = f.tbr * 125 * dur end
  return s
end

local function has_v(f) return f.vcodec and f.vcodec ~= "none" end
local function has_a(f) return f.acodec and f.acodec ~= "none" end

local function better(a, b, key)
  if not a then return true end
  local ha, hb = b.height or 0, a.height or 0
  if key == "h" and ha ~= hb then return ha > hb end
  return (b.tbr or b.abr or 0) > (a.tbr or a.abr or 0)
end

-- Estimated bytes (and picked height) of what the format selector below would download.
local function estimate(info, q, audio_only, ff)
  local dur = info.duration
  local fm = info.formats or {}
  local ba, bv, bp
  local h = height_limit(q)
  for _, f in ipairs(fm) do
    if has_a(f) and not has_v(f) and better(ba, f) then ba = f end
    if has_v(f) and (f.height or 0) <= h then
      if better(bv, f, "h") then bv = f end
      if has_a(f) and better(bp, f, "h") then bp = f end
    end
  end
  if audio_only then
    local f = ba or bp
    return f and fsize(f, dur), nil
  end
  if ff and bv then
    local s = fsize(bv, dur)
    if not has_a(bv) and ba then
      local sa = fsize(ba, dur)
      s = s and sa and s + sa or nil
    end
    return s, bv.height
  end
  if bp then return fsize(bp, dur), bp.height end
  if info.filesize or info.filesize_approx then return info.filesize or info.filesize_approx, info.height end
  return nil, nil
end

local function format_args(ff)
  if o.mode == "Audio only" then
    local a = { "-f", "ba/b" }
    local af = o["audio-format"]
    if af ~= "Original" then
      if ff then
        a = concat(a, { "-x", "--audio-format", af })
      else
        toast("Converting to " .. af .. " needs ffmpeg; keeping the original audio", 5)
      end
    end
    return a
  end
  local h = height_limit(o.quality)
  local lim = h == math.huge and "" or "[height<=" .. h .. "]"
  if ff then return { "-f", "bv*" .. lim .. "+ba/b" .. lim .. "/b" } end
  -- Without ffmpeg yt-dlp cannot merge: single files with video and audio only.
  return { "-f", "b" .. lim .. "/b" }
end

-- ---------- ffmpeg ----------

local FF_DIR = DATA .. SEP .. "ffmpeg"
local FF_MANAGED = FF_DIR .. SEP .. "ffmpeg" .. EXE
local BTBN = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/"
local OSXEXPERTS = "https://www.osxexperts.net/"
local FF_UPDATE_EVERY = 30 * 86400

local function rmtree(dir, cb)
  if windows then run({ "cmd", "/c", "rmdir", "/s", "/q", dir }, cb or function() end)
  else run({ "rm", "-rf", dir }, cb or function() end) end
end

local function read_file(p)
  local f = io.open(p, "r")
  if not f then return nil end
  local s = f:read("*a")
  f:close()
  return s
end

local function write_file(p, s)
  local f = io.open(p, "w")
  if f then f:write(s) f:close() end
end

local function ff_probe(exe, cb)
  run({ exe, "-version" }, function(r)
    cb(r.status == 0 and ((r.stdout or ""):match("ffmpeg version (%S+)") or "?") or nil)
  end)
end

-- System ffmpeg (never inside the Flatpak: the runtime has none and PATH holds no host tools).
local function find_system_ffmpeg(cb)
  if FLATPAK or os.getenv("VIVI_TEST_NO_SYSTEM_FFMPEG") then return cb(nil) end
  local list, seen = {}, {}
  local function add(dir)
    if not dir or dir == "" or dir == FF_DIR or dir == BIN_DIR then return end
    local p = dir .. SEP .. "ffmpeg" .. EXE
    if not seen[p] and utils.file_info(p) then seen[p] = true list[#list + 1] = p end
  end
  for dir in (os.getenv("PATH") or ""):gmatch(windows and "[^;]+" or "[^:]+") do add(dir) end
  if not windows then
    for _, d in ipairs({ "/usr/bin", "/usr/local/bin", "/opt/homebrew/bin", "/opt/local/bin", "/snap/bin" }) do add(d) end
  end
  local i = 0
  local function nxt()
    i = i + 1
    if not list[i] then return cb(nil) end
    ff_probe(list[i], function(v)
      if v then
        local dir = list[i]:match("^(.*)[/\\][^/\\]+$")
        cb({ kind = "system", path = list[i], dir = dir, version = v })
      else nxt() end
    end)
  end
  nxt()
end

-- Bytes of a URL (HEAD, following redirects), or nil.
local function remote_size(url, cb)
  run({ "curl", "-sIL", url }, function(r)
    local n
    for v in (r.stdout or ""):gmatch("[Cc]ontent%-[Ll]ength:%s*(%d+)") do n = tonumber(v) end
    cb(n)
  end)
end

-- What to download for this computer: cb({ files = { {url, sha, member, name, archive} }, id }) or cb(nil, err).
-- BtbN: one archive (sha of the archive) with <top>/bin/ffmpeg and ffprobe.
-- osxexperts: one zip per program (sha of the binary inside).
local function ff_source(cb)
  local function btbn(plat, ext)
    run({ "curl", "-fsSL", BTBN .. "checksums.sha256" }, function(r)
      if r.status ~= 0 then return cb(nil, "cannot reach the BtbN ffmpeg builds") end
      local best, bestv, bestsha
      for sha, name in (r.stdout or ""):gmatch("(%x+)%s+%*?([^\r\n]+)") do
        local v = name:match("^ffmpeg%-n([%d%.]+)%-latest%-" .. plat .. "%-gpl%-[%d%.]+" .. ext:gsub("%.", "%%.") .. "$")
        if v then
          local a, b = v:match("^(%d+)%.?(%d*)")
          local num = tonumber(a) * 1000 + (tonumber(b) or 0)
          if not bestv or num > bestv then best, bestv, bestsha = name, num, sha:lower() end
        end
      end
      if not best then return cb(nil, "no ffmpeg build for " .. plat) end
      local top = best:sub(1, #best - #ext)
      cb({ id = bestsha, files = { {
        url = BTBN .. best, sha = bestsha, archive = true,
        members = { top .. "/bin/ffmpeg" .. EXE, top .. "/bin/ffprobe" .. EXE },
      } } })
    end)
  end
  if windows then
    local a = (os.getenv("PROCESSOR_ARCHITEW6432") or os.getenv("PROCESSOR_ARCHITECTURE") or ""):upper()
    return btbn(a == "ARM64" and "winarm64" or "win64", ".zip")
  end
  run({ "uname", "-sm" }, function(u)
    local s = u.stdout or ""
    if s:match("Linux") and s:match("x86_64") then return btbn("linux64", ".tar.xz") end
    if s:match("Linux") and (s:match("aarch64") or s:match("arm64")) then return btbn("linuxarm64", ".tar.xz") end
    if not s:match("Darwin") then return cb(nil, "no ffmpeg build for " .. s:gsub("%s+$", "")) end
    local arm = s:match("arm64") ~= nil
    run({ "curl", "-fsSL", OSXEXPERTS }, function(r)
      local page = r.stdout or ""
      local text = page:gsub("<[^>]+>", " ")
      local sums = { FFmpeg = {}, ffprobe = {} }
      for prog, sha in text:gmatch("SHA256 checksum of (%w+) file%s*:%s*(%x+)") do
        if sums[prog] and #sha == 64 then table.insert(sums[prog], sha:lower()) end
      end
      local files = {}
      for _, prog in ipairs({ "ffmpeg", "ffprobe" }) do
        local url = page:match('href="(https://www%.osxexperts%.net/' .. prog .. '%d*' .. (arm and "arm" or "intel") .. '%.zip)"')
        -- The page lists each program's arm64 checksum first, then Intel.
        local sha = sums[prog == "ffmpeg" and "FFmpeg" or "ffprobe"][arm and 1 or 2]
        if not url or not sha then return cb(nil, "cannot read the osxexperts.net ffmpeg page") end
        files[#files + 1] = { url = url, sha = sha, archive = false, members = { prog } }
      end
      cb({ id = files[1].sha .. files[2].sha, files = files })
    end)
  end)
end

-- Download, verify and install the managed ffmpeg + ffprobe. cb(ff | nil, err)
-- Archives (BtbN) are checked before unpacking, osxexperts binaries after.
local function install_ffmpeg(src, cb)
  local tmp = FF_DIR .. SEP .. ".x"
  local function fail(msg) rmtree(tmp) cb(nil, msg) end
  local function finish()
    for _, prog in ipairs({ "ffmpeg", "ffprobe" }) do
      local found
      for _, f in ipairs(src.files) do
        for _, m in ipairs(f.members) do
          local pth = tmp .. SEP .. m:gsub("/", SEP)
          if m:sub(-#(prog .. EXE) - 1) == "/" .. prog .. EXE or m == prog .. EXE then
            if utils.file_info(pth) then found = pth end
          end
        end
      end
      if not found then return fail(prog .. " missing from the download") end
      local dst = FF_DIR .. SEP .. prog .. EXE
      os.remove(dst)
      local ok, e = os.rename(found, dst)
      if not ok then return fail(e) end
    end
    rmtree(tmp)
    write_file(FF_DIR .. SEP .. "source", src.id)
    write_file(FF_DIR .. SEP .. "last-update", tostring(os.time()))
    local function probe_new()
      ff_probe(FF_MANAGED, function(v)
        if not v then return cb(nil, "the downloaded ffmpeg does not run") end
        cb({ kind = "managed", path = FF_MANAGED, dir = FF_DIR, version = v })
      end)
    end
    if windows then return probe_new() end
    run({ "chmod", "755", FF_MANAGED, FF_DIR .. "/ffprobe" }, probe_new)
  end
  local function verify(file, want, nxt)
    sha256(file, function(got)
      if not got then return fail("no sha256 tool (sha256sum/shasum/certutil) to verify ffmpeg") end
      if got ~= want then return fail("ffmpeg checksum mismatch, not installed") end
      nxt()
    end)
  end
  local function fetch(i)
    local f = src.files[i]
    if not f then return finish() end
    local dl = tmp .. SEP .. "download" .. i .. (f.url:match("%.tar%.xz$") and ".tar.xz" or ".zip")
    run({ "curl", "-fsSL", "--retry", "2", "-o", dl, f.url }, function(r)
      if r.status ~= 0 then return fail("ffmpeg download failed: " .. (r.stderr or "")) end
      local function unpack(after)
        run(concat({ "tar", "-xf", dl, "-C", tmp }, f.members), function(x)
          os.remove(dl)
          if x.status ~= 0 then return fail("cannot unpack ffmpeg: " .. (x.stderr or "")) end
          after()
        end)
      end
      if f.archive then
        verify(dl, f.sha, function() unpack(function() fetch(i + 1) end) end)
      else
        unpack(function() verify(tmp .. SEP .. f.members[1], f.sha, function() fetch(i + 1) end) end)
      end
    end)
  end
  rmtree(tmp, function()
    mkdir(tmp, function()
      -- Announce the download size first (HEAD requests).
      local n, total = 0, 0
      for _, f in ipairs(src.files) do
        remote_size(f.url, function(sz)
          total = total + (sz or 0)
          n = n + 1
          if n == #src.files then
            toast("Downloading ffmpeg (" .. human(total) .. ") for merging / converting…", 6)
            fetch(1)
          end
        end)
      end
    end)
  end)
end

local ffm = nil        -- { kind = "system"|"managed", path, dir, version }
local ff_waiting = nil

-- ffmpeg if available; with `need`, download the managed copy when there is none.
local function get_ffmpeg(need, cb)
  if ffm then return cb(ffm) end
  if ff_waiting then ff_waiting[#ff_waiting + 1] = { need, cb } return end
  ff_waiting = { { need, cb } }
  local function finish(f, err)
    ffm = f
    local w = ff_waiting
    ff_waiting = nil
    local needers_failed = false
    for _, x in ipairs(w) do
      if not f and x[1] and err then needers_failed = true end
      x[2](f)
    end
    if needers_failed then toast(err, 8) end
  end
  find_system_ffmpeg(function(sys)
    if sys then return finish(sys) end
    if utils.file_info(FF_MANAGED) then
      return ff_probe(FF_MANAGED, function(v)
        if v then return finish({ kind = "managed", path = FF_MANAGED, dir = FF_DIR, version = v }) end
        finish(nil)
      end)
    end
    local any_need = false
    for _, x in ipairs(ff_waiting) do any_need = any_need or x[1] end
    if not any_need then return finish(nil) end
    ff_source(function(src, e)
      if not src then return finish(nil, e) end
      mkdir(FF_DIR, function() install_ffmpeg(src, function(f, e2) finish(f, e2) end) end)
    end)
  end)
end

-- Re-download the managed ffmpeg when the published build changed.
local function update_ffmpeg(quiet)
  if not utils.file_info(FF_MANAGED) then
    if not quiet then toast("No managed ffmpeg installed (it is downloaded when needed)") end
    return
  end
  ff_source(function(src, e)
    write_file(FF_DIR .. SEP .. "last-update", tostring(os.time()))
    if not src then if not quiet then toast("ffmpeg update: " .. e, 6) end return end
    if (read_file(FF_DIR .. SEP .. "source") or "") == src.id then
      if not quiet then toast("ffmpeg is up to date (" .. (ffm and ffm.version or "managed") .. ")") end
      return
    end
    install_ffmpeg(src, function(f, e2)
      if f then
        if not ffm or ffm.kind == "managed" then ffm = f end
        toast("ffmpeg updated to " .. f.version, 5)
      else
        toast("ffmpeg update failed: " .. e2, 6)
      end
    end)
  end)
end

local function maybe_update_ffmpeg()
  if not o["auto-update"] or not utils.file_info(FF_MANAGED) then return end
  local t = tonumber(read_file(FF_DIR .. SEP .. "last-update") or "") or 0
  if os.time() - t > FF_UPDATE_EVERY then update_ffmpeg(true) end
end

-- Whether the current settings need ffmpeg (and may download it).
local function needs_ffmpeg()
  if o.mode == "Audio only" then return o["audio-format"] ~= "Original" end
  return height_limit(o.quality) >= 1080
end

local QUALITIES = { "Best available", "2160p", "1440p", "1080p", "720p", "480p", "360p" }

local function size_list(info, ff)
  local parts, seen = {}, {}
  for _, q in ipairs(QUALITIES) do
    local s, h = estimate(info, q, false, ff or height_limit(q) >= 1080)
    local key = tostring(h) .. tostring(s)
    if h and not seen[key] then
      seen[key] = true
      parts[#parts + 1] = (q == "Best available" and "best " .. h .. "p" or q) .. " ≈ " .. human(s)
    end
  end
  parts[#parts + 1] = "audio ≈ " .. human((estimate(info, nil, true, ff)))
  return table.concat(parts, " · ")
end

local function current_url()
  local u = (o.url or ""):gsub("^%s+", ""):gsub("%s+$", "")
  if u ~= "" then return u end
  local p = mp.get_property("path", "")
  if p:match("^https?://") then return p end
  return nil
end

-- One video's info (first entry for a playlist URL), cached per URL.
local function video_info(y, url, cb)
  if info_cache[url] then return cb(info_cache[url]) end
  ytdlp_json(y, { "--no-playlist", url }, function(j, e)
    if not j then return cb(nil, e) end
    if j._type == "playlist" then
      local first = j.entries and j.entries[1]
      if not first then return cb(nil, "empty playlist") end
      return ytdlp_json(y, { "--no-playlist", first.webpage_url or first.url }, function(v, e2)
        if v then v._playlist = j.title end
        info_cache[url] = v
        cb(v, e2)
      end)
    end
    info_cache[url] = j
    cb(j)
  end)
end

-- ---------- actions ----------

local function entry_url(e)
  local u = e.webpage_url or e.url
  if u and u:match("^https?://") then return u end
  if e.ie_key == "Youtube" or (e.extractor_key or "") == "Youtube" then
    return "https://www.youtube.com/watch?v=" .. (e.id or u or "")
  end
  return u
end

local function append(url, title)
  if title and title ~= "" then
    mp.command_native({ "loadfile", url, "append-play", -1, "force-media-title=%" .. #title .. "%" .. title })
  else
    mp.commandv("loadfile", url, "append-play")
  end
end

local function add_to_playlist(url)
  url = url or current_url()
  if not url then return toast("Enter a video or playlist URL in the YouTube plugin settings (or press Alt+U)") end
  with_ytdlp(function(y)
    toast("Reading " .. url .. "…", 2)
    ytdlp_json(y, { "--flat-playlist", url }, function(j, e)
      if not j then return toast("YouTube: " .. e, 6) end
      if j._type == "playlist" then
        local n = 0
        for _, it in ipairs(j.entries or {}) do
          local u = entry_url(it)
          if u then append(u, it.title) n = n + 1 end
        end
        toast(string.format("Added %d entries from %s", n, j.title or "the playlist"), 4)
      else
        if j.formats then info_cache[url] = j end
        append(j.webpage_url or url, j.title)
        toast("Added " .. (j.title or url), 3)
      end
    end)
  end)
end

local function show_sizes()
  local url = current_url()
  if not url then return toast("Enter a video URL first") end
  with_ytdlp(function(y)
    toast("Checking sizes…", 2)
    get_ffmpeg(false, function(ff)
      video_info(y, url, function(info, e)
        if not info then return toast("YouTube: " .. e, 6) end
        local head = info._playlist and ("First video of " .. info._playlist .. ": ") or ""
        toast(head .. size_list(info, ff ~= nil) .. (ff and "" or " (below 1080p without ffmpeg: single-file formats)"), 10)
      end)
    end)
  end)
end

local function default_folder()
  return HOME .. SEP .. "Downloads" .. SEP .. "Vivi"
end

local log_timer, last_progress

local function read_progress(log)
  local f = io.open(log, "r")
  if not f then return end
  local s = f:read("*a")
  f:close()
  local line
  for l in s:gmatch("%[vivi%][^\r\n]*") do line = l end
  if not line or line == last_progress then return end
  last_progress = line
  local pct, total, eta, idx, n, title = line:match("^%[vivi%] *([^|]*)|([^|]*)|([^|]*)|([^|]*)|([^|]*)|(.*)$")
  if not pct then return end
  local msg = "Downloading " .. pct:gsub("%s", "")
  if total ~= "" and total ~= "NA" and total ~= "N/A" then msg = msg .. " of " .. total:gsub("^%s+", "") end
  if eta ~= "" and eta ~= "NA" and eta ~= "Unknown" then msg = msg .. ", ETA " .. eta:gsub("%s", "") end
  if idx ~= "NA" and n ~= "NA" and idx ~= "" then msg = msg .. " (" .. idx .. "/" .. n .. ")" end
  if title ~= "" then msg = msg .. " · " .. title end
  mp.commandv("script-message", "vivi-toast", msg)
end

local function download()
  if job then return toast("A download is already running (Cancel stops it)") end
  local url = current_url()
  if not url then return toast("Enter a video or playlist URL first") end
  with_ytdlp(function(y)
    get_ffmpeg(needs_ffmpeg(), function(ffi)
      local ff = ffi ~= nil
      if not ff and needs_ffmpeg() then
        return toast("Download needs ffmpeg, which could not be installed; pick a lower quality or Original audio", 8)
      end
      local folder = (o.folder or ""):gsub("^%s+", ""):gsub("%s+$", "")
      if folder == "" then folder = default_folder() end
      folder = folder:gsub("^~", HOME)
      local function start()
        mkdir(folder, function()
          local log = (os.getenv("TMPDIR") or os.getenv("TEMP") or "/tmp") .. SEP .. "vivi-ytdlp-" .. mp.get_property("pid", "0") .. ".log"
          os.remove(log)
          last_progress = nil
          local args = concat(y.cmd, concat({
            "--newline", "--progress", "--no-colors", "--yes-playlist",
            "--progress-template",
            "download:[vivi] %(progress._percent_str)s|%(progress._total_bytes_str,progress._total_bytes_estimate_str)s|%(progress._eta_str)s|%(info.playlist_index)s|%(info.n_entries)s|%(info.title)s",
            -- Audio gets its own name: -x deletes its source file, which could be a video
            -- downloaded earlier under the same name.
            "-P", folder, "-o", "%(playlist_index&{} - |)s%(title)s [%(id)s]" .. (o.mode == "Audio only" and " (audio)" or "") .. ".%(ext)s",
          }, concat(format_args(ff), ffi and { "--ffmpeg-location", ffi.dir, url } or { url })))
          if not windows then
            args = concat({ "sh", "-c", 'log=$1; shift; exec "$@" >"$log" 2>&1', "sh", log }, args)
            log_timer = mp.add_periodic_timer(2, function() read_progress(log) end)
          end
          local me
          me = run(args, function(r)
            if job == me then job = nil end
            if log_timer then log_timer:kill() log_timer = nil end
            local out = (r.stderr or "")
            local f = io.open(log, "r")
            if f then out = out .. f:read("*a") f:close() end
            os.remove(log)
            if r.killed_by_us then
              toast("Download cancelled")
            elseif r.status == 0 then
              toast("Downloaded to " .. folder, 6)
            else
              toast("Download failed: " .. (out:match("ERROR: ([^\n]+)") or r.error_string or "yt-dlp error"), 8)
            end
          end, false)
          job = me
        end)
      end
      -- Estimated size first (single videos; playlists say how many entries).
      video_info(y, url, function(info)
        local what = o.mode == "Audio only" and "audio" or o.quality
        if info then
          local s = estimate(info, o.quality, o.mode == "Audio only", ff)
          local pl = info._playlist and (" each, playlist " .. info._playlist) or ""
          toast(string.format("Downloading %s (%s ≈ %s%s) to %s", info.title or url, what, human(s), pl, folder), 5)
        else
          toast("Downloading " .. url .. " to " .. folder, 4)
        end
        start()
      end)
    end)
  end)
end

local function cancel()
  if not job then return toast("No download is running") end
  mp.abort_async_command(job)
end

local function show_info()
  ensure(function(y, err)
    if not y then return toast(err or "No yt-dlp available", 6) end
    get_ffmpeg(false, function(f)
      local fs = f and string.format("%s ffmpeg %s (%s)", f.kind, f.version, f.path)
        or "no ffmpeg yet (downloaded for 1080p+ or audio conversion)"
      toast(string.format("Using %s yt-dlp %s (%s)%s; %s", y.kind, y.version, y.path,
        recent(y.version) and "" or " — old, YouTube may fail", fs), 10)
    end)
  end)
end

local function paste()
  local t = mp.get_property("clipboard/text", "") or ""
  t = t:gsub("^%s+", ""):gsub("%s+$", "")
  if not t:match("^https?://%S+$") then
    return toast("No URL in the clipboard (paste it into the YouTube plugin settings instead)")
  end
  o.url = t
  mp.commandv("script-message", "vivi-setting", "youtube", "url", t)
  add_to_playlist(t)
end

local last_source = o.source
options.read_options(o, "youtube", function()
  if o.source ~= last_source then
    last_source = o.source
    ytdlp = nil
    ffmpeg = nil
  end
end)
last_source = o.source

mp.register_script_message("add", function() add_to_playlist() end)
mp.register_script_message("sizes", show_sizes)
mp.register_script_message("download", download)
mp.register_script_message("cancel", cancel)
mp.register_script_message("update", function()
  toast("Updating yt-dlp…", 2)
  self_update(false)
end)
mp.register_script_message("info", show_info)
mp.register_script_message("update-ffmpeg", function()
  toast("Checking for a newer ffmpeg…", 2)
  update_ffmpeg(false)
end)
mp.register_script_message("pick-folder", function()
  mp.commandv("script-message", "vivi-pick-folder", "youtube", "folder")
end)
mp.register_script_message("folder-picked", function(p)
  o.folder = p
  toast("Downloads go to " .. p)
end)
mp.add_forced_key_binding("Alt+u", "youtube-paste", paste)
mp.register_event("shutdown", function() if job then mp.abort_async_command(job) end end)

-- An existing managed copy is handed to ytdl_hook right away (streaming works before any
-- plugin action), and checked for updates.
if o.source ~= "System" and utils.file_info(MANAGED) then
  ensure(function() end)
end
mp.add_timeout(30, maybe_update_ffmpeg)

-- For tests: `script-message-to youtube selftest <json file>` prints size estimates.
mp.register_script_message("selftest", function(file)
  local f = io.open(file, "r")
  local j = utils.parse_json(f:read("*a"))
  f:close()
  print("sizes(ffmpeg): " .. size_list(j, true))
  print("sizes(no ffmpeg): " .. size_list(j, false))
  print("fmt(1080p,ff): " .. table.concat(format_args(true), " "))
  print("needs_ffmpeg(1080p video)=" .. tostring(needs_ffmpeg()))
  if file:match("ffmpeg") then
    get_ffmpeg(true, function(f) print("ffmpeg: " .. (f and (f.kind .. " " .. f.version .. " " .. f.path) or "none")) mp.command("quit") end)
    return
  end
  print("recent(2000.01.01)=" .. tostring(recent("2000.01.01")) .. " recent(today)=" .. tostring(recent(os.date("%Y.%m.%d"))))
  mp.command("quit")
end)
