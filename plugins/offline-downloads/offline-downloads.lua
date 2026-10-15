-- @name Offline Downloads
-- @version 1.0.0
-- @author Vivi
-- @description Download queue for YouTube/yt-dlp sites, direct music files (Jamendo, Internet Archive, …) and radio streams (Vivi plugin package).
--
-- Keys: Alt+d   add the URL in the clipboard (or the playing URL) to the queue
--       Alt+D   show the queue (Alt+Shift+d)
-- Script messages: add, add-current, enqueue <url>, show, pause, cancel, retry,
--                  clear-done, open-folder, pick-folder, info.
--
-- One item downloads at a time; the queue (pending / active / paused / done / failed)
-- is kept in plugin-data/offline-downloads/queue.json and survives restarts.
--
-- How an URL is fetched:
--   * direct media files (.mp3 .ogg .flac .opus .m4a .mp4 .mkv …, Jamendo/Internet
--     Archive download links, or any URL whose Content-Type is audio/video with a known
--     length): curl, resumable (curl -C -). Progress = bytes on disk / Content-Length.
--   * live radio (Icecast/Shoutcast, audio without Content-Length, .pls/.m3u lists):
--     recorded for the "Radio recording length" setting with curl --max-time.
--     HLS (.m3u8) needs ffmpeg (system, or one another Vivi plugin already downloaded).
--   * anything else (YouTube, Jamendo/IA pages, SoundCloud, Bandcamp, …): yt-dlp.
--
-- yt-dlp: a system yt-dlp is used, else the copy the YouTube & yt-dlp plugin manages
-- (plugin-data/youtube/bin). This plugin never downloads its own yt-dlp or ffmpeg; it
-- only reuses ffmpeg found on the system or already installed by the YouTube plugin
-- (plugin-data/youtube/ffmpeg) or the shared media build (plugin-data/shared/ffmpeg-media).
local mp = require "mp"
local options = require "mp.options"
local utils = require "mp.utils"

local o = { url = "", folder = "", ["radio-minutes"] = "30", ["video-quality"] = "Best single file" }
options.read_options(o, "offline-downloads")

local windows = package.config:sub(1, 1) == "\\"
local SEP = windows and "\\" or "/"
local EXE = windows and ".exe" or ""
local HOME = os.getenv("HOME") or os.getenv("USERPROFILE") or "."
local DATA_HOME = os.getenv("XDG_DATA_HOME")
if not DATA_HOME or DATA_HOME == "" then
  DATA_HOME = windows and (os.getenv("APPDATA") or HOME) or (HOME .. "/.local/share")
end
local PLUGIN_DATA = DATA_HOME .. SEP .. "vivi" .. SEP .. "plugin-data"
local DIR = PLUGIN_DATA .. SEP .. "offline-downloads"
local QUEUE = DIR .. SEP .. "queue.json"
local FLATPAK = utils.file_info("/.flatpak-info") ~= nil
local TMP = os.getenv("TMPDIR") or os.getenv("TEMP") or "/tmp"

local function run(args, cb)
  return mp.command_native_async({ name = "subprocess", args = args, playback_only = false,
    capture_stdout = true, capture_stderr = true }, function(_, r) cb(r or {}) end)
end

local function toast(msg, secs)
  mp.osd_message(msg, secs or 4)
  mp.commandv("script-message", "vivi-toast", msg)
end

local function mkdir(dir, cb)
  if windows then run({ "cmd", "/c", "if not exist \"" .. dir .. "\" mkdir \"" .. dir .. "\"" }, cb)
  else run({ "mkdir", "-p", dir }, cb) end
end

local function human(n)
  if not n then return "?" end
  if n >= 1073741824 then return string.format("%.2f GB", n / 1073741824) end
  if n >= 1048576 then return string.format("%.1f MB", n / 1048576) end
  return string.format("%.0f KB", n / 1024)
end

local function trim(s) return ((s or ""):gsub("^%s+", ""):gsub("%s+$", "")) end

local function folder()
  local f = trim(o.folder)
  if f == "" then f = HOME .. SEP .. "Downloads" .. SEP .. "Vivi" end
  return (f:gsub("^~", HOME))
end

-- ---------- classification (pure) ----------

local MEDIA_EXT = { mp3 = 1, ogg = 1, oga = 1, opus = 1, flac = 1, m4a = 1, aac = 1, wav = 1, wma = 1,
  mp4 = 1, m4v = 1, mkv = 1, webm = 1, mov = 1, avi = 1, ts = 1, mka = 1 }
local LIST_EXT = { pls = 1, m3u = 1 }

local function url_ext(url)
  local path = url:gsub("[?#].*$", "")
  return (path:match("%.([%w]+)$") or ""):lower()
end

-- "file" | "stream" | "hls" | "list" | "ytdlp" from the URL alone (headers may refine it).
local function classify(url)
  local ext = url_ext(url)
  if ext == "m3u8" then return "hls" end
  if LIST_EXT[ext] then return "list" end
  if MEDIA_EXT[ext] then return "file" end
  -- Jamendo's storage and archive.org /download/ URLs are files even without extension.
  if url:match("^https?://[%w%.%-]*storage%.jamendo%.com/") or url:match("^https?://[%w%.%-]*mp3l%.jamendo%.com/")
    or url:match("^https?://archive%.org/download/") then return "file" end
  return "ytdlp"
end

-- Refine with response headers (curl -sIL output): live audio has no length or ICY headers.
local function classify_headers(kind, head)
  head = head or ""
  local ctype, len
  for v in head:gmatch("[Cc]ontent%-[Tt]ype:%s*([^\r\n;]+)") do ctype = v:lower() end
  for v in head:gmatch("[Cc]ontent%-[Ll]ength:%s*(%d+)") do len = tonumber(v) end
  local icy = head:lower():find("\nicy%-") ~= nil or head:find("^ICY") ~= nil
  if ctype and ctype:match("apple%.mpegurl") then return "hls", nil end
  if ctype and (ctype:match("mpegurl") or ctype:match("x%-scpls")) then return "list", nil end
  if icy then return "stream", nil end
  if ctype and (ctype:match("^audio/") or ctype:match("^video/") or ctype == "application/ogg") then
    if not len or len == 0 then return "stream", nil end
    return "file", len
  end
  return kind, len
end

-- First http(s) URL in a .pls/.m3u body.
local function first_list_url(body)
  for line in (body or ""):gmatch("[^\r\n]+") do
    local u = line:match("^File%d+=(https?://%S+)") or line:match("^%s*(https?://%S+)")
    if u then return u end
  end
end

local function safe_name(s)
  s = (s or ""):gsub("[%c<>:\"/\\|%?%*]", "_"):gsub("^%s+", ""):gsub("[%s%.]+$", "")
  if s == "" then s = "download" end
  return s:sub(1, 150)
end

local function unescape(s) return (s:gsub("%%(%x%x)", function(h) return string.char(tonumber(h, 16)) end)) end

local function file_name(url, kind)
  local path = url:gsub("[?#].*$", "")
  local base = unescape(path:match("^%a+://[^/]+/(.*)$") or ""):match("([^/]+)/*$") or ""
  local host = url:match("^%a+://([^/:]+)") or "stream"
  if kind == "stream" then
    local ext = url_ext(url)
    local stem = safe_name(base ~= "" and (base:gsub("%.[%w]+$", "")) or host)
    return stem .. " " .. os.date("%Y-%m-%d %H%M") .. "." .. (MEDIA_EXT[ext] and ext or "mp3")
  end
  if kind == "hls" then return safe_name(host) .. " " .. os.date("%Y-%m-%d %H%M") .. ".ts" end
  if base == "" then base = host end
  base = safe_name(base)
  if not MEDIA_EXT[(base:match("%.([%w]+)$") or ""):lower()] then base = base .. ".mp3" end
  return base
end

-- yt-dlp --progress-template lines -> percent, total text, title (last line wins).
local function parse_progress(s)
  local line
  for l in (s or ""):gmatch("%[vivi%][^\r\n]*") do line = l end
  if not line then return nil end
  local pct, total, title = line:match("^%[vivi%] *([^|]*)|([^|]*)|(.*)$")
  if not pct then return nil end
  total = trim(total)
  if total == "NA" or total == "N/A" then total = "" end
  return tonumber((pct:gsub("[%s%%]", ""))), total, trim(title)
end

-- ---------- queue ----------

local queue = {}      -- { id, url, kind, state, file, size, done, total, pct, total_text, title, error, added }
local active          -- { item, job, timer, t0, aborted = "pause"|"cancel" }
local next_id = 1

local function save()
  mkdir(DIR, function()
    local f = io.open(QUEUE .. ".tmp", "w")
    if not f then return end
    f:write(utils.format_json({ items = queue }))
    f:close()
    os.remove(QUEUE)
    os.rename(QUEUE .. ".tmp", QUEUE)
  end)
end

local function load()
  local f = io.open(QUEUE, "r")
  if not f then return end
  local v = utils.parse_json(f:read("*a") or "")
  f:close()
  queue = type(v) == "table" and type(v.items) == "table" and v.items or {}
  for _, it in ipairs(queue) do
    if it.state == "active" then it.state = "pending" end -- interrupted by quitting
    if (it.id or 0) >= next_id then next_id = it.id + 1 end
  end
end

local function counts()
  local c = { pending = 0, active = 0, paused = 0, done = 0, failed = 0 }
  for _, it in ipairs(queue) do c[it.state] = (c[it.state] or 0) + 1 end
  return c
end

local function describe(it)
  local name = it.title or (it.file and it.file:match("([^/\\]+)$")) or it.url
  local s = "[" .. it.state .. "] " .. name
  if it.state == "active" or it.state == "paused" then
    if it.pct then s = s .. string.format(" %.0f%%", it.pct) end
    if it.total and it.total > 0 then s = s .. " of " .. human(it.total)
    elseif it.total_text and it.total_text ~= "" then s = s .. " of " .. it.total_text
    elseif it.done and it.done > 0 then s = s .. " " .. human(it.done) end
  elseif it.state == "done" and it.size then s = s .. " (" .. human(it.size) .. ")"
  elseif it.state == "failed" and it.error then s = s .. ": " .. it.error:sub(1, 80) end
  return s
end

local function show()
  if #queue == 0 then return toast("The download queue is empty") end
  local c, lines = counts(), {}
  lines[1] = string.format("Downloads: %d pending, %d active, %d paused, %d done, %d failed → %s",
    c.pending, c.active, c.paused, c.done, c.failed, folder())
  for i = math.max(1, #queue - 11), #queue do lines[#lines + 1] = describe(queue[i]) end
  toast(table.concat(lines, "\n"), 10)
end

-- ---------- tools (found, never downloaded) ----------

local function find(names, extra_dirs)
  local dirs = {}
  if not FLATPAK and not os.getenv("VIVI_TEST_NO_SYSTEM_TOOLS") then
    for d in (os.getenv("PATH") or ""):gmatch(windows and "[^;]+" or "[^:]+") do dirs[#dirs + 1] = d end
    if not windows then
      for _, d in ipairs({ HOME .. "/.local/bin", "/usr/local/bin", "/usr/bin", "/opt/homebrew/bin", "/snap/bin" }) do dirs[#dirs + 1] = d end
    end
  end
  for _, d in ipairs(extra_dirs or {}) do dirs[#dirs + 1] = d end
  for _, d in ipairs(dirs) do
    for _, n in ipairs(names) do
      local p = d .. SEP .. n .. EXE
      if utils.file_info(p) then return p, d end
    end
  end
end

local YT_BIN = PLUGIN_DATA .. SEP .. "youtube" .. SEP .. "bin"
local FF_DIRS = { PLUGIN_DATA .. SEP .. "youtube" .. SEP .. "ffmpeg", PLUGIN_DATA .. SEP .. "shared" .. SEP .. "ffmpeg-media" }

local function ytdlp() return (find({ "yt-dlp" }, { YT_BIN })) end
local function ffmpeg() return find({ "ffmpeg" }, FF_DIRS) end

-- ---------- running ----------

local start_next

local function finish(it, ok, err)
  local a = active
  if a and a.timer then a.timer:kill() end
  active = nil
  if a and a.aborted == "pause" then
    it.state = "paused"
  elseif a and a.aborted == "cancel" then
    for i, x in ipairs(queue) do if x == it then table.remove(queue, i) break end end
    if it.file and it.kind ~= "ytdlp" then os.remove(it.file) end
    toast("Download cancelled")
  elseif ok then
    it.state, it.error, it.pct = "done", nil, 100
    local fi = it.file and utils.file_info(it.file)
    it.size = fi and fi.size or it.size
    toast("Downloaded: " .. describe(it), 5)
  else
    it.state, it.error = "failed", trim(err or "error"):match("([^\n]*)$")
    toast("Download failed: " .. it.error, 8)
  end
  save()
  start_next()
end

local function curl_job(it, max_time)
  local args = { "curl", "-fL", "--retry", "2", "-s", "-A", "Vivi", "-o", it.file }
  if max_time then
    args[#args + 1] = "--max-time"
    args[#args + 1] = tostring(max_time)
  else
    args[#args + 1] = "-C"
    args[#args + 1] = "-"
  end
  args[#args + 1] = it.url
  local t0 = active.t0
  active.timer = mp.add_periodic_timer(1, function()
    local fi = utils.file_info(it.file)
    it.done = fi and fi.size or 0
    if it.total and it.total > 0 then it.pct = math.min(99, it.done / it.total * 100)
    elseif max_time then it.pct = math.min(99, (os.time() - t0) / max_time * 100) end
  end)
  active.job = run(args, function(r)
    -- curl exits 28 when --max-time ends a recording: that is the normal end.
    local ok = r.status == 0 or (max_time ~= nil and r.status == 28 and utils.file_info(it.file) ~= nil)
    finish(it, ok, (r.stderr or "") ~= "" and r.stderr or ("curl exit " .. tostring(r.status)))
  end)
end

local function ytdlp_job(it)
  local y = ytdlp()
  if not y then
    return finish(it, false, "no yt-dlp: install yt-dlp or the YouTube & yt-dlp plugin (its managed copy is reused here)")
  end
  local ff, ffdir = ffmpeg()
  local q = o["video-quality"]
  local fmt
  if q == "Audio only" then fmt = "ba/b"
  elseif ff and q ~= "Best single file" then
    local h = q:match("(%d+)p")
    fmt = h and ("bv*[height<=" .. h .. "]+ba/b[height<=" .. h .. "]/b") or "bv*+ba/b"
  else fmt = "b" end
  local log = TMP .. SEP .. "vivi-offline-" .. mp.get_property("pid", "0") .. ".log"
  os.remove(log)
  os.remove(log .. ".path")
  local args = { y, "--newline", "--no-colors", "--no-playlist", "-f", fmt, "-P", folder(),
    "-o", "%(title).150B [%(id)s].%(ext)s", "--print-to-file", "after_move:filepath", log .. ".path",
    "--progress-template", "download:[vivi] %(progress._percent_str)s|%(progress._total_bytes_str,progress._total_bytes_estimate_str)s|%(info.title)s" }
  if ffdir then args[#args + 1] = "--ffmpeg-location" args[#args + 1] = ffdir end
  args[#args + 1] = it.url
  if not windows then
    local w = { "sh", "-c", 'log=$1; shift; exec "$@" >"$log" 2>&1', "sh", log }
    for _, x in ipairs(args) do w[#w + 1] = x end
    args = w
  end
  active.timer = mp.add_periodic_timer(1, function()
    local f = io.open(log, "r")
    if not f then return end
    local pct, total, title = parse_progress(f:read("*a"))
    f:close()
    if pct then it.pct, it.total_text = pct, total end
    if title and title ~= "" then it.title = title end
  end)
  active.job = run(args, function(r)
    local out = (r.stdout or "") .. (r.stderr or "")
    local f = io.open(log, "r")
    if f then out = out .. f:read("*a") f:close() end
    local pf = io.open(log .. ".path", "r")
    local path = pf and trim(pf:read("*a")):match("([^\r\n]+)$")
    if pf then pf:close() end
    os.remove(log)
    os.remove(log .. ".path")
    if path and utils.file_info(path) then it.file = path end
    finish(it, r.status == 0, out:match("ERROR: ([^\n]+)") or r.error_string or "yt-dlp error")
  end)
end

local SITES = { "youtube%.com", "youtu%.be", "vimeo%.com", "soundcloud%.com", "bandcamp%.com",
  "twitch%.tv", "dailymotion%.com", "mixcloud%.com" }
local function known_site(url)
  local host = (url:match("^%a+://([^/:]+)") or ""):lower()
  for _, s in ipairs(SITES) do
    if host:match("^" .. s .. "$") or host:match("%." .. s .. "$") then return true end
  end
  return false
end

local function begin(it)
  active = { item = it, t0 = os.time() }
  it.state, it.error = "active", nil
  save()
  mkdir(folder(), function()
    -- Well-known video/music sites skip the header probe; other URLs may be radio or files.
    if it.kind == "ytdlp" and known_site(it.url) then return ytdlp_job(it) end
    run({ "curl", "-sIL", "-A", "Vivi", "--max-time", "15", it.url }, function(h)
      if active and active.aborted then return finish(it, false) end
      local kind, len = classify_headers(it.kind, h.stdout)
      if kind == "list" then
        return run({ "curl", "-sL", "--max-time", "15", it.url }, function(b)
          local u = first_list_url(b.stdout)
          if not u then return finish(it, false, "no stream URL in the playlist file") end
          it.url = u
          it.kind = classify(u) == "hls" and "hls" or "stream"
          active = nil
          begin(it)
        end)
      end
      it.kind = kind
      if kind == "ytdlp" then return ytdlp_job(it) end
      it.file = it.file or (folder() .. SEP .. file_name(it.url, kind))
      local secs = math.floor((tonumber(o["radio-minutes"]) or 30) * 60)
      if kind == "hls" then
        local ff = ffmpeg()
        if not ff then return finish(it, false, "HLS streams need ffmpeg (install ffmpeg)") end
        local t0 = active.t0
        active.timer = mp.add_periodic_timer(1, function() it.pct = math.min(99, (os.time() - t0) / secs * 100) end)
        active.job = run({ ff, "-hide_banner", "-loglevel", "error", "-y", "-i", it.url, "-t", tostring(secs), "-c", "copy", it.file },
          function(r) finish(it, r.status == 0, r.stderr) end)
        return
      end
      it.total = len
      curl_job(it, kind == "stream" and secs or nil)
    end)
  end)
end

start_next = function()
  if active then return end
  for _, it in ipairs(queue) do
    if it.state == "pending" then return begin(it) end
  end
end

local function enqueue(url)
  url = trim(url)
  if not url:match("^https?://%S+$") then return toast("Not an http(s) URL: " .. url:sub(1, 80)) end
  for _, it in ipairs(queue) do
    if it.url == url and it.state ~= "done" and it.state ~= "failed" then return toast("Already in the queue") end
  end
  local it = { id = next_id, url = url, kind = classify(url), state = "pending", added = os.time() }
  next_id = next_id + 1
  queue[#queue + 1] = it
  save()
  toast(string.format("Queued (%s): %s — %d waiting", it.kind == "ytdlp" and "yt-dlp" or it.kind,
    url:sub(1, 80), counts().pending), 4)
  start_next()
  return it
end

local function abort(how)
  active.aborted = how
  if active.job then mp.abort_async_command(active.job) end
end

local function pause()
  if active then
    abort("pause")
    return toast("Paused (Pause / Resume again continues)")
  end
  local any = false
  for _, it in ipairs(queue) do if it.state == "paused" then it.state = "pending" any = true end end
  if not any then return toast("Nothing to pause or resume") end
  save()
  toast("Resumed")
  start_next()
end

local function cancel()
  if active then return abort("cancel") end
  local n = 0
  for i = #queue, 1, -1 do
    if queue[i].state == "pending" or queue[i].state == "paused" then table.remove(queue, i) n = n + 1 end
  end
  save()
  toast(n > 0 and ("Removed " .. n .. " waiting download(s)") or "Nothing to cancel")
end

local function retry()
  local n = 0
  for _, it in ipairs(queue) do
    if it.state == "failed" then it.state, it.error, it.kind = "pending", nil, classify(it.url) n = n + 1 end
  end
  save()
  toast(n > 0 and ("Retrying " .. n .. " download(s)") or "No failed downloads")
  start_next()
end

local function clear_done()
  for i = #queue, 1, -1 do if queue[i].state == "done" then table.remove(queue, i) end end
  save()
  toast("Finished downloads removed from the list (files are kept)")
end

local function open_folder()
  local f = folder()
  mkdir(f, function()
    local cmd = windows and { "explorer", f } or (utils.file_info("/System/Library") and { "open", f } or { "xdg-open", f })
    run(cmd, function() end)
  end)
end

local function add_clipboard_or_current()
  local t = trim(mp.get_property("clipboard/text", "") or "")
  if t:match("^https?://%S+$") then return enqueue(t) end
  local p = mp.get_property("path", "") or ""
  if p:match("^https?://") then return enqueue(p) end
  toast("Copy a URL first (or paste it into the Offline Downloads settings)")
end

local function info()
  toast(string.format("yt-dlp: %s\nffmpeg: %s\nFolder: %s",
    ytdlp() or "none (install yt-dlp or the YouTube & yt-dlp plugin)",
    ffmpeg() or "none (single-file formats only; HLS recording unavailable)", folder()), 8)
end

load()
options.read_options(o, "offline-downloads", function() end)
mp.register_script_message("add", function() enqueue(o.url) end)
mp.register_script_message("add-current", function()
  local p = mp.get_property("path", "") or ""
  if p:match("^https?://") then enqueue(p) else toast("The playing item is not an online URL") end
end)
mp.register_script_message("enqueue", enqueue)
mp.register_script_message("show", show)
mp.register_script_message("pause", pause)
mp.register_script_message("cancel", cancel)
mp.register_script_message("retry", retry)
mp.register_script_message("clear-done", clear_done)
mp.register_script_message("open-folder", open_folder)
mp.register_script_message("info", info)
mp.register_script_message("pick-folder", function()
  mp.commandv("script-message", "vivi-pick-folder", "offline-downloads", "folder")
end)
mp.register_script_message("folder-picked", function(p) o.folder = p toast("Downloads go to " .. p) end)
mp.add_forced_key_binding("Alt+d", "offline-add", add_clipboard_or_current)
mp.add_forced_key_binding("Alt+D", "offline-show", show)
mp.register_event("shutdown", function() if active then abort("pause") end end)
mp.add_timeout(3, start_next)

if VIVI_TEST then
  return { classify = classify, classify_headers = classify_headers, first_list_url = first_list_url,
    file_name = file_name, parse_progress = parse_progress, enqueue = enqueue, retry = retry,
    clear_done = clear_done, known_site = known_site, queue = function() return queue end, describe = describe, human = human,
    set_active = function(a) active = a end }
end
