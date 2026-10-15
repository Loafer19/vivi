-- @name Scrobbler (Last.fm & ListenBrainz)
-- @version 1.1.0
-- @author Vivi
-- @description Scrobbles to Last.fm and/or ListenBrainz with an offline queue (Vivi plugin package).
--
-- Rules (Last.fm/ListenBrainz convention): a track is scrobbled once it has been played
-- for half its length or 4 minutes, whichever comes first; tracks shorter than
-- 30 seconds are ignored. Time is counted from actual playback (seeking does not count).
-- Artist and title come from the file tags (mpv metadata).
--
-- ListenBrainz: POST <api>/1/submit-listens with `Authorization: Token <token>`.
-- Last.fm: signed POSTs to https://ws.audioscrobbler.com/2.0/ (track.updateNowPlaying,
-- track.scrobble). Last.fm has no per-user tokens without an API account, so the user
-- enters an API key + shared secret (free, https://www.last.fm/api/account/create) and
-- connects once: "Connect Last.fm" opens the browser authorisation page, "Finish
-- connecting" turns the approved token into a session key that is saved as a setting.
--
-- Everything that cannot be sent (offline, server error) is appended to
-- $XDG_DATA_HOME/vivi/plugin-data/scrobbler/queue.jsonl (ListenBrainz) and
-- lastfm-queue.jsonl (Last.fm) and sent in batches later (at start, after every
-- successful scrobble, every 10 minutes and on "flush").
-- Script messages: flush, status, lastfm_connect, lastfm_finish.
local mp = require "mp"
local options = require "mp.options"
local utils = require "mp.utils"

local VERSION = "1.1.0"
local o = {
  token = "", now_playing = true, audio_only = true, api = "https://api.listenbrainz.org",
  lastfm_key = "", lastfm_secret = "", lastfm_session = "",
}
local LASTFM = "https://ws.audioscrobbler.com/2.0/"

local home = os.getenv("HOME") or os.getenv("USERPROFILE") or "."
local data = os.getenv("XDG_DATA_HOME")
if not data or data == "" then
  data = os.getenv("APPDATA") or (home .. "/.local/share")
end
local dir = data .. "/vivi/plugin-data/scrobbler"
local QUEUES = { lb = dir .. "/queue.jsonl", lastfm = dir .. "/lastfm-queue.jsonl" }
local windows = package.config:sub(1, 1) == "\\"

local cur = nil -- { meta, duration, played, sent, started_at }
local last_pos = nil
local sending = {}
local last_error = {}
local sent_count = { lb = 0, lastfm = 0 }
local lastfm_token = nil -- pending desktop-auth token

local function log(...) mp.msg.info(...) end
local function trim(s) return ((s or ""):gsub("^%s+", ""):gsub("%s+$", "")) end
local function token() return trim(o.token) end

local function toast(msg, secs)
  mp.osd_message(msg, secs or 4)
  mp.commandv("script-message", "vivi-toast", msg)
end

-- ---------- MD5 (Last.fm signatures), pure Lua ----------

local band, bor, bxor, bnot, lshift, rshift
do
  local ok, b = pcall(require, "bit") -- LuaJIT
  if not ok then ok, b = pcall(require, "bit32") end -- Lua 5.2
  if not ok and bit32 then ok, b = true, bit32 end
  if ok and b then
    local norm = function(x) return x % 4294967296 end
    band = function(a, c) return norm(b.band(a, c)) end
    bor = function(a, c) return norm(b.bor(a, c)) end
    bxor = function(a, c) return norm(b.bxor(a, c)) end
    bnot = function(a) return norm(b.bnot(a)) end
    lshift = function(a, n) return norm(b.lshift(a, n)) end
    rshift = function(a, n) return norm(b.rshift(norm(a), n)) end
  else -- Lua 5.3+: native integer operators (loaded as text so 5.1 can parse this file)
    band, bor, bxor, bnot, lshift, rshift = load([[
      local m = 0xffffffff
      return function(a, b) return (a & b) & m end, function(a, b) return (a | b) & m end,
        function(a, b) return (a ~ b) & m end, function(a) return (~a) & m end,
        function(a, n) return (a << n) & m end, function(a, n) return ((a & m) >> n) end
    ]])()
  end
end

local function md5(msg)
  local K, S = {}, {
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
    5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
    4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
    6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
  }
  for i = 0, 63 do K[i] = math.floor(math.abs(math.sin(i + 1)) * 4294967296) end
  local function add(a, b) return (a + b) % 4294967296 end
  local function rol(x, n) return bor(lshift(x, n), rshift(x, 32 - n)) end
  local len = #msg
  msg = msg .. "\128" .. string.rep("\0", (55 - len) % 64)
  local bits = len * 8
  for _ = 1, 8 do
    msg = msg .. string.char(bits % 256)
    bits = math.floor(bits / 256)
  end
  local a0, b0, c0, d0 = 0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476
  for chunk = 1, #msg, 64 do
    local M = {}
    for i = 0, 15 do
      local p = chunk + i * 4
      local w0, w1, w2, w3 = msg:byte(p, p + 3)
      M[i] = w0 + w1 * 256 + w2 * 65536 + w3 * 16777216
    end
    local A, B, C, D = a0, b0, c0, d0
    for i = 0, 63 do
      local F, g
      if i < 16 then F = bor(band(B, C), band(bnot(B), D)); g = i
      elseif i < 32 then F = bor(band(D, B), band(bnot(D), C)); g = (5 * i + 1) % 16
      elseif i < 48 then F = bxor(bxor(B, C), D); g = (3 * i + 5) % 16
      else F = bxor(C, bor(B, bnot(D))); g = (7 * i) % 16 end
      F = add(add(add(F, A), K[i]), M[g])
      A, D, C = D, C, B
      B = add(B, rol(F, S[i + 1]))
    end
    a0, b0, c0, d0 = add(a0, A), add(b0, B), add(c0, C), add(d0, D)
  end
  local out = {}
  for _, v in ipairs({ a0, b0, c0, d0 }) do
    for _ = 1, 4 do
      out[#out + 1] = string.format("%02x", v % 256)
      v = math.floor(v / 256)
    end
  end
  return table.concat(out)
end

-- ---------- queue files ----------

local function ensure_dir()
  if windows then
    utils.subprocess({ args = { "cmd", "/c", "mkdir", (dir:gsub("/", "\\")) }, playback_only = false })
  else
    utils.subprocess({ args = { "mkdir", "-p", dir }, playback_only = false })
  end
end

local function read_queue(svc)
  local q = {}
  local f = io.open(QUEUES[svc], "r")
  if not f then return q end
  for line in f:lines() do
    local item = utils.parse_json(line)
    if item and (item.listened_at or item.timestamp) then q[#q + 1] = item end
  end
  f:close()
  return q
end

local function write_queue(svc, q)
  ensure_dir()
  local file = QUEUES[svc]
  local tmp = file .. ".tmp"
  local f = io.open(tmp, "w")
  if not f then return end
  for _, item in ipairs(q) do f:write(utils.format_json(item), "\n") end
  f:close()
  os.remove(file)
  os.rename(tmp, file)
end

local function enqueue(svc, items)
  local q = read_queue(svc)
  for _, l in ipairs(items) do q[#q + 1] = l end
  write_queue(svc, q)
  log(string.format("%s: queued %d, %d waiting", svc, #items, #q))
end

-- ---------- HTTP ----------

-- curl with the body on stdin; cb(code | nil, body, err)
local function http(args, body, cb)
  local a = { "curl", "-sS", "-m", "20", "-o", "-", "-w", "\n%{http_code}", "-A", "Vivi-scrobbler/" .. VERSION }
  for _, x in ipairs(args) do a[#a + 1] = x end
  mp.command_native_async({
    name = "subprocess", args = a, stdin_data = body, capture_stdout = true,
    capture_stderr = true, playback_only = false,
  }, function(success, res, err)
    local out = (res and res.stdout) or ""
    local code = tonumber(out:match("(%d%d%d)%s*$") or "")
    if not success or not res or res.status ~= 0 or not code then
      return cb(nil, "", "network error: " .. ((res and res.stderr) or err or "curl failed"))
    end
    cb(code, (out:gsub("\n%d+%s*$", "")), nil)
  end)
end

-- ListenBrainz: cb(ok, retryable, message)
local function lb_submit(body, cb)
  if token() == "" then return cb(false, true, "no ListenBrainz token set") end
  local url = (o.api:gsub("/+$", "")) .. "/1/submit-listens"
  http({ "-H", "Authorization: Token " .. token(), "-H", "Content-Type: application/json",
    "--data-binary", "@-", url }, utils.format_json(body), function(code, out, err)
    if not code then return cb(false, true, err) end
    if code >= 200 and code < 300 then return cb(true, false, "ok") end
    local j = utils.parse_json(out) or {}
    -- 4xx other than 429 will not get better by retrying the same payload,
    -- except 401 (token fixed later): keep those queued too.
    local retry = code >= 500 or code == 429 or code == 401
    cb(false, retry, string.format("HTTP %d %s", code, j.error or ""))
  end)
end

local function urlencode(s)
  return (tostring(s):gsub("[^%w%-_%.~]", function(c) return string.format("%%%02X", c:byte()) end))
end

-- Last.fm api_sig: md5 of the sorted "<key><value>" pairs (without format) + secret.
local function lastfm_sig(params, secret)
  local keys = {}
  for k in pairs(params) do if k ~= "format" and k ~= "callback" then keys[#keys + 1] = k end end
  table.sort(keys)
  local s = {}
  for _, k in ipairs(keys) do s[#s + 1] = k .. params[k] end
  return md5(table.concat(s) .. secret)
end

local function form(params)
  local keys = {}
  for k in pairs(params) do keys[#keys + 1] = k end
  table.sort(keys)
  local s = {}
  for _, k in ipairs(keys) do s[#s + 1] = urlencode(k) .. "=" .. urlencode(params[k]) end
  return table.concat(s, "&")
end

local function lastfm_ready() return trim(o.lastfm_key) ~= "" and trim(o.lastfm_secret) ~= "" and trim(o.lastfm_session) ~= "" end

-- Last.fm error codes worth retrying later: 8 operation failed, 11 offline, 16 temporary,
-- 29 rate limit, 9 invalid session (fixed by reconnecting).
local LASTFM_RETRY = { [8] = true, [9] = true, [11] = true, [16] = true, [29] = true }

-- Signed Last.fm call; cb(ok, retryable, message, json)
local function lastfm_call(params, cb, unsigned_session)
  params.api_key = trim(o.lastfm_key)
  if not unsigned_session and params.sk == nil then params.sk = trim(o.lastfm_session) end
  params.api_sig = lastfm_sig(params, trim(o.lastfm_secret))
  params.format = "json"
  http({ "-H", "Content-Type: application/x-www-form-urlencoded", "--data-binary", "@-", LASTFM },
    form(params), function(code, out, err)
      if not code then return cb(false, true, err) end
      local j = utils.parse_json(out) or {}
      if code >= 200 and code < 300 and not j.error then return cb(true, false, "ok", j) end
      local e = tonumber(j.error)
      local retry = (e and LASTFM_RETRY[e]) or code >= 500 or (not e and code ~= 400)
      cb(false, retry, string.format("Last.fm %s %s", e or ("HTTP " .. code), j.message or ""), j)
    end)
end

-- track.scrobble form fields for up to 50 queued items (array notation).
local function lastfm_batch(items)
  local p = { method = "track.scrobble" }
  for i, it in ipairs(items) do
    local n = "[" .. (i - 1) .. "]"
    p["artist" .. n] = it.artist
    p["track" .. n] = it.track
    p["timestamp" .. n] = tostring(it.timestamp)
    if it.album then p["album" .. n] = it.album end
    if it.duration then p["duration" .. n] = tostring(it.duration) end
    if it.mbid then p["mbid" .. n] = it.mbid end
  end
  return p
end

-- ---------- scrobbling ----------

local flush

local function payload_of(meta, listened_at)
  local l = { track_metadata = meta }
  if listened_at then l.listened_at = listened_at end
  return l
end

local function lastfm_item(c)
  local m = c.meta
  return {
    artist = m.artist_name, track = m.track_name, album = m.release_name,
    timestamp = c.started_at, mbid = m.additional_info.recording_mbid,
    duration = c.duration > 0 and math.floor(c.duration) or nil,
  }
end

local function services()
  local s = {}
  if token() ~= "" then s[#s + 1] = "lb" end
  if lastfm_ready() then s[#s + 1] = "lastfm" end
  return s
end

local function scrobble(c)
  for _, svc in ipairs(services()) do
    local item, send
    if svc == "lb" then
      item = payload_of(c.meta, c.started_at)
      send = function(cb) lb_submit({ listen_type = "single", payload = { item } }, cb) end
    else
      item = lastfm_item(c)
      send = function(cb) lastfm_call(lastfm_batch({ item }), cb) end
    end
    send(function(ok, retry, msg)
      if ok then
        sent_count[svc] = sent_count[svc] + 1
        last_error[svc] = nil
        log(svc .. ": scrobbled " .. c.meta.artist_name .. " - " .. c.meta.track_name)
        flush(false, svc)
      else
        last_error[svc] = msg
        mp.msg.warn(svc .. ": scrobble failed: " .. msg)
        if retry then enqueue(svc, { item }) end
      end
    end)
  end
end

local function flush_one(svc, manual)
  if sending[svc] then return end
  local q = read_queue(svc)
  if #q == 0 then return 0 end
  local ready = svc == "lb" and token() ~= "" or svc == "lastfm" and lastfm_ready()
  if not ready then return 0 end
  local max = svc == "lb" and 100 or 50
  local batch = {}
  for i = 1, math.min(#q, max) do batch[i] = q[i] end
  sending[svc] = true
  local function done(ok, retry, msg)
    sending[svc] = false
    if ok or not retry then
      local rest = {}
      for i = #batch + 1, #q do rest[#rest + 1] = q[i] end
      -- Keep anything queued while we were sending.
      local now = read_queue(svc)
      for i = #q + 1, #now do rest[#rest + 1] = now[i] end
      write_queue(svc, rest)
      if ok then
        sent_count[svc] = sent_count[svc] + #batch
        log(string.format("%s: sent %d queued", svc, #batch))
        if manual then mp.osd_message(string.format("Scrobbler: sent %d queued (%s)", #batch, svc == "lb" and "ListenBrainz" or "Last.fm")) end
        if #rest > 0 then flush_one(svc, manual) end
      else
        mp.msg.warn(svc .. ": dropped " .. #batch .. " queued: " .. msg)
      end
    else
      last_error[svc] = msg
      if manual then mp.osd_message("Scrobbler: " .. msg) end
    end
  end
  if svc == "lb" then
    lb_submit({ listen_type = "import", payload = batch }, done)
  else
    lastfm_call(lastfm_batch(batch), done)
  end
  return #batch
end

flush = function(manual, only)
  local n = 0
  for _, svc in ipairs({ "lb", "lastfm" }) do
    if not only or only == svc then n = n + (flush_one(svc, manual) or 0) end
  end
  if manual and n == 0 then
    if #services() == 0 then mp.osd_message("Scrobbler: set a ListenBrainz token or connect Last.fm first")
    else mp.osd_message("Scrobbler: nothing queued") end
  end
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

-- Seconds of playback after which a track of `duration` seconds is scrobbled (nil: never).
local function threshold(duration)
  if not duration or duration < 30 then return nil end
  return math.min(duration / 2, 240)
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
  if not o.now_playing then return end
  if token() ~= "" then
    lb_submit({ listen_type = "playing_now", payload = { payload_of(meta) } }, function(ok, _, msg)
      if not ok then mp.msg.verbose("ListenBrainz now playing failed: " .. msg) end
    end)
  end
  if lastfm_ready() then
    local p = { method = "track.updateNowPlaying", artist = artist, track = title }
    if meta.release_name then p.album = meta.release_name end
    if duration > 0 then p.duration = tostring(math.floor(duration)) end
    lastfm_call(p, function(ok, _, msg)
      if not ok then mp.msg.verbose("Last.fm now playing failed: " .. msg) end
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
  local need = threshold(cur.duration)
  if need and cur.played >= need then
    cur.sent = true
    scrobble(cur)
  end
end

-- ---------- Last.fm desktop authorisation ----------

local function open_url(url)
  local args = windows and { "cmd", "/c", "start", "", url }
    or (os.getenv("VIVI_TEST_NO_BROWSER") and { "true" })
    or { "sh", "-c", 'command -v xdg-open >/dev/null && exec xdg-open "$1" || exec open "$1"', "sh", url }
  mp.command_native_async({ name = "subprocess", args = args, playback_only = false }, function() end)
end

local function lastfm_connect()
  if trim(o.lastfm_key) == "" or trim(o.lastfm_secret) == "" then
    toast("Enter your Last.fm API key and shared secret first (free at last.fm/api/account/create)", 8)
    return
  end
  lastfm_call({ method = "auth.getToken" }, function(ok, _, msg, j)
    if not ok or not j or not j.token then return toast("Last.fm: " .. msg, 6) end
    lastfm_token = j.token
    open_url("https://www.last.fm/api/auth/?api_key=" .. urlencode(trim(o.lastfm_key)) .. "&token=" .. urlencode(j.token))
    toast("Allow Vivi in the browser, then press \"Finish connecting\"", 10)
  end, true)
end

local function lastfm_finish()
  if not lastfm_token then return toast("Press \"Connect Last.fm\" first") end
  lastfm_call({ method = "auth.getSession", token = lastfm_token }, function(ok, _, msg, j)
    if not ok or not j or not j.session or not j.session.key then
      return toast("Last.fm: " .. msg .. " (did you allow access in the browser?)", 6)
    end
    lastfm_token = nil
    o.lastfm_session = j.session.key
    mp.commandv("script-message", "vivi-setting", "scrobbler", "lastfm_session", j.session.key)
    toast("Last.fm connected as " .. tostring(j.session.name or "?"))
    flush(false, "lastfm")
  end, true)
end

mp.register_event("file-loaded", start_track)
mp.observe_property("playback-time", "number", on_time)
mp.register_event("seek", function() last_pos = nil end)

options.read_options(o, "scrobbler", function() end)

mp.register_script_message("flush", function() flush(true) end)
mp.register_script_message("lastfm_connect", lastfm_connect)
mp.register_script_message("lastfm_finish", lastfm_finish)
mp.register_script_message("status", function()
  local lines = {}
  lines[1] = string.format("ListenBrainz: %s, %d sent, %d queued", token() == "" and "no token" or "ready",
    sent_count.lb, #read_queue("lb"))
  lines[2] = string.format("Last.fm: %s, %d sent, %d queued", lastfm_ready() and "connected"
    or (trim(o.lastfm_key) == "" and "not set up" or "not connected"), sent_count.lastfm, #read_queue("lastfm"))
  if cur then
    local need = threshold(cur.duration)
    lines[#lines + 1] = string.format("%s - %s: %d/%ds", cur.meta.artist_name, cur.meta.track_name,
      math.floor(cur.played), math.floor(need or 0))
  end
  for svc, e in pairs(last_error) do lines[#lines + 1] = "Last error (" .. svc .. "): " .. e end
  mp.osd_message(table.concat(lines, "\n"), 6)
end)

mp.add_timeout(5, function() flush(false) end)
mp.add_periodic_timer(600, function() flush(false) end)

if VIVI_TEST then
  return { md5 = md5, lastfm_sig = lastfm_sig, lastfm_batch = lastfm_batch, threshold = threshold,
    form = form, lastfm_item = lastfm_item }
end
