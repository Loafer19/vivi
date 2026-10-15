-- @name ListenBrainz Scrobbler
-- @version 1.0.1
-- @author Vivi
-- @description Scrobbles to ListenBrainz with an offline queue (Vivi plugin package).
--
-- Rules (Last.fm/ListenBrainz convention): a track is scrobbled once it has been played
-- for half its length or 4 minutes, whichever comes first; tracks shorter than
-- 30 seconds are ignored. Time is counted from actual playback (seeking does not count).
-- Artist and title come from the file tags (mpv metadata).
--
-- Network: POST <api>/1/submit-listens with `Authorization: Token <token>` via curl.
-- Listens that cannot be sent (offline, server error) are appended to
-- $XDG_DATA_HOME/vivi/plugin-data/scrobbler/queue.jsonl and sent in batches later
-- (at start, after every successful submit, every 10 minutes and on "flush").
-- Script messages: flush, status.
local mp = require "mp"
local options = require "mp.options"
local utils = require "mp.utils"

local VERSION = "1.0.0"
local o = { token = "", now_playing = true, audio_only = true, api = "https://api.listenbrainz.org" }

local home = os.getenv("HOME") or os.getenv("USERPROFILE") or "."
local data = os.getenv("XDG_DATA_HOME")
if not data or data == "" then
  data = os.getenv("APPDATA") or (home .. "/.local/share")
end
local dir = data .. "/vivi/plugin-data/scrobbler"
local queue_file = dir .. "/queue.jsonl"

local cur = nil -- { meta, duration, played, sent, started_at }
local last_pos = nil
local sending = false
local last_error = ""
local sent_count = 0

local function log(...) mp.msg.info(...) end

local function token() return ((o.token or ""):gsub("^%s+", ""):gsub("%s+$", "")) end

local function ensure_dir()
  local sep = package.config:sub(1, 1)
  if sep == "\\" then
    utils.subprocess({ args = { "cmd", "/c", "mkdir", (dir:gsub("/", "\\")) }, playback_only = false })
  else
    utils.subprocess({ args = { "mkdir", "-p", dir }, playback_only = false })
  end
end

local function read_queue()
  local q = {}
  local f = io.open(queue_file, "r")
  if not f then return q end
  for line in f:lines() do
    local item = utils.parse_json(line)
    if item and item.listened_at then q[#q + 1] = item end
  end
  f:close()
  return q
end

local function write_queue(q)
  ensure_dir()
  local tmp = queue_file .. ".tmp"
  local f = io.open(tmp, "w")
  if not f then return end
  for _, item in ipairs(q) do f:write(utils.format_json(item), "\n") end
  f:close()
  os.remove(queue_file)
  os.rename(tmp, queue_file)
end

local function enqueue(listens)
  local q = read_queue()
  for _, l in ipairs(listens) do q[#q + 1] = l end
  write_queue(q)
  log(string.format("queued %d listen(s), %d waiting", #listens, #q))
end

-- POST a submit-listens body; cb(ok, retryable, message)
local function submit(body, cb)
  if token() == "" then
    cb(false, true, "no ListenBrainz token set")
    return
  end
  local url = (o.api:gsub("/+$", "")) .. "/1/submit-listens"
  mp.command_native_async({
    name = "subprocess",
    args = {
      "curl", "-sS", "-m", "20", "-o", "-", "-w", "\n%{http_code}",
      "-H", "Authorization: Token " .. token(),
      "-H", "Content-Type: application/json",
      "-A", "Vivi-scrobbler/" .. VERSION,
      "--data-binary", "@-", url,
    },
    stdin_data = utils.format_json(body),
    capture_stdout = true,
    capture_stderr = true,
    playback_only = false,
  }, function(success, res, err)
    local out = (res and res.stdout) or ""
    local code = tonumber(out:match("(%d%d%d)%s*$") or "")
    if not success or not res or res.status ~= 0 or not code then
      cb(false, true, "network error: " .. ((res and res.stderr) or err or "curl failed"))
    elseif code >= 200 and code < 300 then
      cb(true, false, "ok")
    else
      local j = utils.parse_json((out:gsub("\n%d+%s*$", ""))) or {}
      -- 4xx other than 429 will not get better by retrying the same payload,
      -- except 401 (token fixed later): keep those queued too.
      local retry = code >= 500 or code == 429 or code == 401
      cb(false, retry, string.format("HTTP %d %s", code, j.error or ""))
    end
  end)
end

local flush

local function payload_of(meta, listened_at)
  local l = { track_metadata = meta }
  if listened_at then l.listened_at = listened_at end
  return l
end

local function scrobble(c)
  local listen = payload_of(c.meta, c.started_at)
  submit({ listen_type = "single", payload = { listen } }, function(ok, retry, msg)
    if ok then
      sent_count = sent_count + 1
      last_error = ""
      log("scrobbled " .. c.meta.artist_name .. " - " .. c.meta.track_name)
      flush()
    else
      last_error = msg
      mp.msg.warn("scrobble failed: " .. msg)
      if retry then enqueue({ listen }) end
    end
  end)
end

flush = function(manual)
  if sending then return end
  local q = read_queue()
  if #q == 0 then
    if manual then mp.osd_message("Scrobbler: nothing queued") end
    return
  end
  if token() == "" then
    if manual then mp.osd_message("Scrobbler: set your ListenBrainz token first") end
    return
  end
  local batch = {}
  for i = 1, math.min(#q, 100) do batch[i] = q[i] end
  sending = true
  submit({ listen_type = "import", payload = batch }, function(ok, retry, msg)
    sending = false
    if ok or not retry then
      local rest = {}
      for i = #batch + 1, #q do rest[#rest + 1] = q[i] end
      -- Keep anything queued while we were sending.
      local now = read_queue()
      for i = #q + 1, #now do rest[#rest + 1] = now[i] end
      write_queue(rest)
      if ok then
        sent_count = sent_count + #batch
        log(string.format("sent %d queued listen(s)", #batch))
        if manual then mp.osd_message(string.format("Scrobbler: sent %d queued listen(s)", #batch)) end
        if #rest > 0 then flush(manual) end
      else
        mp.msg.warn("dropped " .. #batch .. " queued listen(s): " .. msg)
      end
    else
      last_error = msg
      if manual then mp.osd_message("Scrobbler: " .. msg) end
    end
  end)
end

local function tag(md, k)
  return md[k] or md[k:upper()] or md[k:sub(1, 1):upper() .. k:sub(2)]
end

local function is_video()
  for _, t in ipairs(mp.get_property_native("track-list") or {}) do
    if t.type == "video" and not t.albumart and not t.image then return true end
  end
  return false
end

local function start_track()
  cur, last_pos = nil, nil
  local md = mp.get_property_native("metadata") or {}
  local artist, title = tag(md, "artist"), tag(md, "title")
  if not artist or not title or artist == "" or title == "" then return end
  if o.audio_only and is_video() then return end
  local duration = mp.get_property_number("duration", 0)
  local meta = {
    artist_name = artist,
    track_name = title,
    additional_info = {
      media_player = "Vivi",
      submission_client = "Vivi scrobbler",
      submission_client_version = VERSION,
    },
  }
  local album = tag(md, "album")
  if album and album ~= "" then meta.release_name = album end
  if duration > 0 then meta.additional_info.duration_ms = math.floor(duration * 1000) end
  local mbid = md["MUSICBRAINZ_TRACKID"] or md["MusicBrainz Track Id"] or md["musicbrainz_trackid"]
  if mbid and mbid ~= "" then meta.additional_info.recording_mbid = mbid end
  cur = { meta = meta, duration = duration, played = 0, sent = false, started_at = os.time() }
  if o.now_playing and token() ~= "" then
    submit({ listen_type = "playing_now", payload = { payload_of(meta) } }, function(ok, _, msg)
      if not ok then mp.msg.verbose("now playing failed: " .. msg) end
    end)
  end
end

-- Count played media time from position changes (seeks and pauses do not count).
local function on_time(_, pos)
  if not cur or cur.sent or not pos then
    last_pos = pos
    return
  end
  if last_pos and not mp.get_property_bool("pause", false) then
    local d = pos - last_pos
    local speed = mp.get_property_number("speed", 1)
    if d > 0 and d <= 2 * math.max(1, speed) then cur.played = cur.played + d end
  end
  last_pos = pos
  if cur.duration >= 30 and cur.played >= math.min(cur.duration / 2, 240) then
    cur.sent = true
    scrobble(cur)
  end
end

mp.register_event("file-loaded", start_track)
mp.observe_property("playback-time", "number", on_time)
mp.register_event("seek", function() last_pos = nil end)

options.read_options(o, "scrobbler", function() end)

mp.register_script_message("flush", function() flush(true) end)
mp.register_script_message("status", function()
  local q = #read_queue()
  local s = string.format("Scrobbler: %s, %d sent this session, %d queued",
    token() == "" and "no token" or "ready", sent_count, q)
  if cur then
    s = s .. string.format("\n%s - %s: %d/%ds", cur.meta.artist_name, cur.meta.track_name,
      math.floor(cur.played), math.floor(math.min(cur.duration / 2, 240)))
  end
  if last_error ~= "" then s = s .. "\nLast error: " .. last_error end
  mp.osd_message(s, 6)
end)

mp.add_timeout(5, function() flush(false) end)
mp.add_periodic_timer(600, function() flush(false) end)
