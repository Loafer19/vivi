-- @name SponsorBlock
-- @version 1.0.0
-- @author Vivi
-- @description Skip SponsorBlock segments in YouTube streams (Vivi plugin package).
--
-- Keys: Alt+k  skip   skip the current (or announced) segment
-- Network: GET <api>/api/skipSegments/<sha256 prefix>?categories=[...] via curl.
local mp = require "mp"
local options = require "mp.options"
local utils = require "mp.utils"

local o = {
  categories = "sponsor selfpromo interaction",
  mode = "Skip automatically",
  chapters = true,
  api = "https://sponsor.ajay.app",
}

local segments = {}   -- { {a, b, category}, ... }
local skipped = {}    -- index -> true (auto mode: skip each once per file)
local announced = nil -- index of the segment offered for Alt+K
local video_id = nil

local function toast(msg, secs)
  mp.osd_message(msg, secs or 3)
  mp.commandv("script-message", "vivi-toast", msg)
end

local NAMES = {
  sponsor = "Sponsor", selfpromo = "Self-promotion", interaction = "Interaction reminder",
  intro = "Intro", outro = "Outro", preview = "Preview", music_offtopic = "Non-music part",
  filler = "Filler",
}

-- YouTube video id from a URL, or nil.
local function youtube_id(url)
  if not url then return nil end
  local patterns = {
    "youtube%.com/watch%?.-v=([%w_-]+)", "youtube%.com/shorts/([%w_-]+)",
    "youtube%.com/live/([%w_-]+)", "youtube%.com/embed/([%w_-]+)",
    "youtube%-nocookie%.com/embed/([%w_-]+)", "youtu%.be/([%w_-]+)",
    "music%.youtube%.com/watch%?.-v=([%w_-]+)",
  }
  for _, p in ipairs(patterns) do
    local id = url:match(p)
    if id and #id == 11 then return id end
  end
  local ytdl = url:match("^ytdl://([%w_-]+)$")
  if ytdl and #ytdl == 11 then return ytdl end
  return nil
end

local function categories()
  local list = {}
  for c in (o.categories or ""):gmatch("[%w_]+") do list[#list + 1] = '"' .. c .. '"' end
  if #list == 0 then list = { '"sponsor"' } end
  return "[" .. table.concat(list, ",") .. "]"
end

local function sha_prefix(id)
  for _, cmd in ipairs({ { "sha256sum" }, { "shasum", "-a", "256" } }) do
    local r = mp.command_native({ name = "subprocess", args = cmd, stdin_data = id,
      capture_stdout = true, capture_stderr = true, playback_only = false })
    if r and r.status == 0 and r.stdout then
      local h = r.stdout:match("^(%x+)")
      if h and #h == 64 then return h:sub(1, 4) end
    end
  end
  return nil
end

local function urlencode(s)
  return (s:gsub("[^%w%-_%.~]", function(c) return string.format("%%%02X", c:byte()) end))
end

-- Segment list for `id` from a skipSegments answer (hash-prefix form or plain array).
local function parse(body, id)
  local v = utils.parse_json(body or "")
  if type(v) ~= "table" then return {} end
  local list = v
  if v[1] and v[1].videoID then
    list = {}
    for _, e in ipairs(v) do if e.videoID == id then list = e.segments or {} end end
  end
  local out = {}
  for _, s in ipairs(list) do
    if type(s.segment) == "table" and (s.actionType == nil or s.actionType == "skip") then
      local a, b = tonumber(s.segment[1]), tonumber(s.segment[2])
      if a and b and b > a then out[#out + 1] = { a, b, s.category or "sponsor" } end
    end
  end
  table.sort(out, function(x, y) return x[1] < y[1] end)
  return out
end

local function add_chapters()
  if not o.chapters or #segments == 0 then return end
  local list = mp.get_property_native("chapter-list") or {}
  for _, s in ipairs(segments) do
    local n = NAMES[s[3]] or s[3]
    list[#list + 1] = { time = s[1], title = n }
    list[#list + 1] = { time = s[2], title = "End of " .. n:lower() }
  end
  table.sort(list, function(x, y) return x.time < y.time end)
  mp.set_property_native("chapter-list", list)
end

local function fetch(id)
  local base = (o.api or "https://sponsor.ajay.app"):gsub("/+$", "")
  local prefix = sha_prefix(id)
  local url
  if prefix then
    url = base .. "/api/skipSegments/" .. prefix .. "?categories=" .. urlencode(categories())
  else
    url = base .. "/api/skipSegments?videoID=" .. id .. "&categories=" .. urlencode(categories())
  end
  mp.command_native_async({ name = "subprocess", playback_only = false, capture_stdout = true,
    capture_stderr = true, args = { "curl", "-sS", "-m", "15", "-A", "Vivi SponsorBlock", url } },
    function(ok, r)
      if id ~= video_id then return end
      if not ok or not r or r.status ~= 0 then
        mp.msg.warn("SponsorBlock request failed: " .. tostring(r and r.stderr))
        return
      end
      segments = parse(r.stdout, id)
      mp.msg.info(string.format("%d segment(s) for %s", #segments, id))
      if #segments > 0 then
        toast(string.format("SponsorBlock: %d segment%s", #segments, #segments == 1 and "" or "s"))
        add_chapters()
      end
    end)
end

local function skip_to(i)
  local s = segments[i]
  if not s then return end
  skipped[i] = true
  announced = nil
  mp.set_property_number("time-pos", s[2])
  toast("Skipped " .. (NAMES[s[3]] or s[3]):lower())
end

local function on_time(_, t)
  if not t or #segments == 0 then return end
  for i, s in ipairs(segments) do
    if t >= s[1] and t < s[2] - 0.5 then
      if o.mode:find("^Skip") then
        if not skipped[i] then skip_to(i) end
      elseif announced ~= i then
        announced = i
        toast((NAMES[s[3]] or s[3]) .. " — press Alt+K to skip", 5)
      end
      return
    end
  end
  announced = nil
end

local function skip_key()
  local t = mp.get_property_number("time-pos")
  if announced then skip_to(announced); return end
  for i, s in ipairs(segments) do
    if t and t >= s[1] and t < s[2] then skip_to(i); return end
  end
  toast("No SponsorBlock segment here")
end

options.read_options(o, "sponsorblock", function() if video_id then fetch(video_id) end end)

mp.register_event("file-loaded", function()
  segments, skipped, announced = {}, {}, nil
  video_id = youtube_id(mp.get_property("path")) or youtube_id(mp.get_property("stream-open-filename"))
  if video_id then fetch(video_id) end
end)
mp.observe_property("time-pos", "number", on_time)
mp.add_forced_key_binding("Alt+k", "skip", skip_key)

-- For tests: `script-message-to sponsorblock selftest` prints parser results and quits.
mp.register_script_message("selftest", function()
  local function show(x) return x and tostring(x) or "nil" end
  print("SELFTEST id " .. show(youtube_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=1")) ..
    " " .. show(youtube_id("https://youtu.be/aqz-KE-bpKQ")) .. " " .. show(youtube_id("https://x.org/a")))
  local p = parse('[{"videoID":"aqz-KE-bpKQ","segments":[{"category":"outro","actionType":"skip","segment":[494.6,618.7]}]}]', "aqz-KE-bpKQ")
  print("SELFTEST parse " .. #p .. " " .. show(p[1] and p[1][3]))
  print("SELFTEST prefix " .. show(sha_prefix("aqz-KE-bpKQ")))
  mp.command("quit")
end)
