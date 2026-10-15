-- Minimal stand-in for mpv's Lua API, so plugin scripts can be loaded by plain LuaJIT /
-- Lua and their pure functions unit-tested (tests/plugins/run.sh). Each test file does
--   local P = dofile("tests/plugins/stub.lua").load("plugins/<id>/<id>.lua", { opts })
-- and gets the table the script returns when VIVI_TEST is set.
local M = {}

-- Tiny JSON (enough for tests): encoder with sorted keys, decoder via a Lua pattern walk.
local function enc(v)
  local t = type(v)
  if t == "nil" then return "null" end
  if t == "boolean" or t == "number" then return tostring(v) end
  if t == "string" then
    return '"' .. v:gsub('[%c"\\]', function(c)
      local m = { ['"'] = '\\"', ["\\"] = "\\\\", ["\n"] = "\\n", ["\t"] = "\\t", ["\r"] = "\\r" }
      return m[c] or string.format("\\u%04x", c:byte())
    end) .. '"'
  end
  if #v > 0 or next(v) == nil then
    local out = {}
    for i = 1, #v do out[i] = enc(v[i]) end
    return "[" .. table.concat(out, ",") .. "]"
  end
  local keys = {}
  for k in pairs(v) do keys[#keys + 1] = tostring(k) end
  table.sort(keys)
  local out = {}
  for _, k in ipairs(keys) do out[#out + 1] = enc(k) .. ":" .. enc(v[k]) end
  return "{" .. table.concat(out, ",") .. "}"
end

local function dec(s)
  local i = 1
  local function ws() i = s:find("[^%s]", i) or #s + 1 end
  local val
  local function str()
    local out = {}
    i = i + 1
    while true do
      local c = s:sub(i, i)
      if c == "" then error("unterminated string") end
      if c == '"' then i = i + 1 break end
      if c == "\\" then
        local n = s:sub(i + 1, i + 1)
        local m = { n = "\n", t = "\t", r = "\r", b = "\b", f = "\f" }
        if n == "u" then
          out[#out + 1] = string.char(tonumber(s:sub(i + 2, i + 5), 16) % 256)
          i = i + 6
        else
          out[#out + 1] = m[n] or n
          i = i + 2
        end
      else
        out[#out + 1] = c
        i = i + 1
      end
    end
    return table.concat(out)
  end
  val = function()
    ws()
    local c = s:sub(i, i)
    if c == "{" then
      local t = {}
      i = i + 1
      ws()
      if s:sub(i, i) == "}" then i = i + 1 return t end
      while true do
        ws()
        local k = str()
        ws()
        i = i + 1 -- :
        t[k] = val()
        ws()
        local d = s:sub(i, i)
        i = i + 1
        if d == "}" then return t end
      end
    elseif c == "[" then
      local t = {}
      i = i + 1
      ws()
      if s:sub(i, i) == "]" then i = i + 1 return t end
      while true do
        t[#t + 1] = val()
        ws()
        local d = s:sub(i, i)
        i = i + 1
        if d == "]" then return t end
      end
    elseif c == '"' then
      return str()
    else
      local lit = s:match("^[%w%.%-+]+", i)
      i = i + #lit
      if lit == "true" then return true elseif lit == "false" then return false
      elseif lit == "null" then return nil end
      return tonumber(lit)
    end
  end
  local ok, v = pcall(val)
  if ok then return v end
  return nil, v
end

function M.load(script, opts)
  M.calls, M.osd, M.messages, M.bindings, M.props = {}, {}, {}, {}, {}
  local mp = {
    msg = { info = function() end, warn = function() end, verbose = function() end,
            error = function() end, debug = function() end },
    get_property = function(k, d) local v = M.props[k] if v == nil then return d end return tostring(v) end,
    get_property_native = function(k, d) local v = M.props[k] if v == nil then return d end return v end,
    get_property_number = function(k, d) return tonumber(M.props[k]) or d end,
    get_property_bool = function(k, d) local v = M.props[k] if v == nil then return d end return v end,
    set_property = function(k, v) M.props[k] = v end,
    set_property_native = function(k, v) M.props[k] = v end,
    set_property_number = function(k, v) M.props[k] = v end,
    commandv = function(...) M.calls[#M.calls + 1] = { ... } end,
    command = function(c) M.calls[#M.calls + 1] = { c } end,
    command_native = function(t) M.calls[#M.calls + 1] = t return { status = 1, stdout = "", stderr = "" } end,
    command_native_async = function(t, cb) M.calls[#M.calls + 1] = t if cb then cb(true, { status = 1, stdout = "", stderr = "" }) end end,
    osd_message = function(m) M.osd[#M.osd + 1] = m end,
    register_event = function() end,
    register_script_message = function(n, f) M.messages[n] = f end,
    observe_property = function() end,
    add_forced_key_binding = function(k, n, f) M.bindings[k] = f end,
    add_key_binding = function(k, n, f) M.bindings[k] = f end,
    add_timeout = function() return { kill = function() end } end,
    add_periodic_timer = function() return { kill = function() end, resume = function() end } end,
    get_time = function() return os.clock() end,
    get_script_name = function() return "test" end,
    create_osd_overlay = function() return { update = function() end, remove = function() end } end,
    get_osd_size = function() return 1280, 720 end,
  }
  local utils = {
    parse_json = dec, format_json = enc,
    file_info = function(p) local f = io.open(p, "r") if f then f:close() return { size = 1, mtime = 0, is_file = true } end end,
    join_path = function(a, b) return a .. "/" .. b end,
    split_path = function(p) local d, f = p:match("^(.*/)([^/]*)$") return d or "", f or p end,
    subprocess = function() return { status = 1, stdout = "" } end,
    readdir = function() return {} end,
  }
  local options = {
    read_options = function(o) for k, v in pairs(opts or {}) do o[k] = v end end,
  }
  package.loaded["mp"] = mp
  package.loaded["mp.utils"] = utils
  package.loaded["mp.options"] = options
  package.loaded["mp.msg"] = mp.msg
  _G.VIVI_TEST = true
  return assert(loadfile(script))()
end

M.json_encode, M.json_decode = enc, dec

local failed = 0
function M.eq(got, want, what)
  if got ~= want then
    failed = failed + 1
    io.stderr:write(string.format("FAIL %s: got %s, want %s\n", what, tostring(got), tostring(want)))
  end
end
function M.ok(cond, what) M.eq(not not cond, true, what) end
function M.done(name)
  if failed > 0 then io.stderr:write(name .. ": " .. failed .. " failure(s)\n") os.exit(1) end
  print(name .. ": ok")
end

return M
