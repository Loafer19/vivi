-- @name YouTube & yt-dlp
-- @version 1.0.0
-- @author Vivi
-- @description List, size and download YouTube (and other yt-dlp) videos and playlists.
--
-- Keys: Alt+u  paste a URL from the clipboard and add its entries to the playlist
--
-- yt-dlp: a system copy is preferred when it is recent (released in the last 90 days):
-- PATH, common install folders and, inside the Flatpak, the host through
-- `flatpak-spawn --host` (needs the org.freedesktop.Flatpak permission, so usually not).
-- Otherwise the plugin downloads its own copy from the official GitHub releases
-- (https://github.com/yt-dlp/yt-dlp/releases/latest), checks it against SHA2-256SUMS and
-- keeps it in $XDG_DATA_HOME/vivi/plugin-data/youtube/bin, where Vivi also looks for it
-- (it appends that folder to PATH, so mpv's ytdl_hook finds it in the Flatpak). Only that
-- managed copy is updated (`yt-dlp -U`, daily when "auto-update" is on). The chosen yt-dlp
-- is also handed to mpv's ytdl_hook (script-opt ytdl_hook-ytdl_path) for streaming.
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

local ytdlp = nil      -- { cmd = {...}, kind = "system"|"host"|"managed", version, path }
local resolving = nil  -- callbacks waiting for detection
local ffmpeg = nil     -- true/false once checked
local job = nil        -- running download
local info_cache = {}  -- url -> yt-dlp JSON (single video)

local function toast(msg, secs)
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
    elseif FLATPAK then
      probe({ "flatpak-spawn", "--host", "yt-dlp" }, function(v)
        cb(v and { cmd = { "flatpak-spawn", "--host", "yt-dlp" }, version = v, path = "host yt-dlp", kind = "host" } or nil)
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

-- Hand the chosen yt-dlp to mpv's ytdl_hook (not possible for the host copy).
local function tell_ytdl_hook(y)
  if y.kind ~= "host" then
    mp.commandv("change-list", "script-opts", "append", "ytdl_hook-ytdl_path=" .. y.path)
  end
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

local function has_ffmpeg(y, cb)
  if ffmpeg ~= nil then return cb(ffmpeg) end
  local cmd = y.kind == "host" and { "flatpak-spawn", "--host", "ffmpeg", "-version" } or { "ffmpeg", "-version" }
  run(cmd, function(r) ffmpeg = r.status == 0 cb(ffmpeg) end)
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

local QUALITIES = { "Best available", "2160p", "1440p", "1080p", "720p", "480p", "360p" }

local function size_list(info, ff)
  local parts, seen = {}, {}
  for _, q in ipairs(QUALITIES) do
    local s, h = estimate(info, q, false, ff)
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
    has_ffmpeg(y, function(ff)
      video_info(y, url, function(info, e)
        if not info then return toast("YouTube: " .. e, 6) end
        local head = info._playlist and ("First video of " .. info._playlist .. ": ") or ""
        toast(head .. size_list(info, ff) .. (ff and "" or " (no ffmpeg: single-file formats only)"), 10)
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
    has_ffmpeg(y, function(ff)
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
            "-P", folder, "-o", "%(playlist_index&{} - |)s%(title)s [%(id)s].%(ext)s",
          }, concat(format_args(ff), { url })))
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
    local what = ({ system = "system", host = "host (flatpak-spawn)", managed = "managed" })[y.kind]
    toast(string.format("Using %s yt-dlp %s (%s)%s", what, y.version, y.path,
      recent(y.version) and "" or " — old, YouTube may fail"), 8)
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

-- For tests: `script-message-to youtube selftest <json file>` prints size estimates.
mp.register_script_message("selftest", function(file)
  local f = io.open(file, "r")
  local j = utils.parse_json(f:read("*a"))
  f:close()
  print("sizes(ffmpeg): " .. size_list(j, true))
  print("sizes(no ffmpeg): " .. size_list(j, false))
  print("fmt(1080p,ff): " .. table.concat(format_args(true), " "))
  print("recent(2000.01.01)=" .. tostring(recent("2000.01.01")) .. " recent(today)=" .. tostring(recent(os.date("%Y.%m.%d"))))
  mp.command("quit")
end)
