-- @name Remote Control
-- @version 1.0.0
-- @author Vivi
-- @description Phone remote control page on the local network (Vivi plugin package).
--
-- The web server is the native helper bin/vivi-remote (installed with the package for
-- this platform). This script turns on mpv's JSON IPC server (input-ipc-server), creates
-- a PIN when none is set (saved with `script-message vivi-setting`), starts the helper and
-- restarts it when the port or PIN changes. The helper drives the player over IPC and
-- puts the address in user-data/remote/url.
local mp = require "mp"
local options = require "mp.options"

local o = { port = "8765", pin = "", pkgdir = "" }
local windows = package.config:sub(1, 1) == "\\"
local job

local function toast(msg)
  mp.commandv("script-message", "vivi-toast", msg)
end

local function ipc_path()
  local cur = mp.get_property("input-ipc-server", "")
  if cur ~= "" then return cur end
  local pid = mp.get_property("pid", "0")
  local p
  if windows then
    p = "\\\\.\\pipe\\vivi-mpv-" .. pid
  else
    p = (os.getenv("XDG_RUNTIME_DIR") or os.getenv("TMPDIR") or "/tmp") .. "/vivi-mpv-" .. pid .. ".sock"
  end
  mp.set_property("input-ipc-server", p)
  return p
end

local function helper(name)
  local dir = o.pkgdir
  if not dir or dir == "" then return nil end
  return dir .. (windows and "\\bin\\" or "/bin/") .. name .. (windows and ".exe" or "")
end

-- 6 random digits (from /dev/urandom when there is one).
local function random_pin()
  local f = io.open("/dev/urandom", "rb")
  if f then
    local b = f:read(4)
    f:close()
    local n = 0
    for i = 1, #b do n = n * 256 + b:byte(i) end
    return string.format("%06d", n % 1000000)
  end
  math.randomseed(os.time() + math.floor(mp.get_time() * 1e6) + tonumber(mp.get_property("pid", "0")))
  return string.format("%06d", math.random(0, 999999))
end

local function stop()
  if job then
    mp.abort_async_command(job)
    job = nil
  end
end

local last = {}

local function start()
  stop()
  last = { port = o.port, pin = o.pin }
  local exe = helper("vivi-remote")
  if not exe then
    toast("Remote Control: helper not found, reinstall the plugin")
    return
  end
  local pin = (o.pin or ""):gsub("%s", "")
  if pin == "" then
    pin = random_pin()
    o.pin = pin
    last.pin = pin
    mp.commandv("script-message", "vivi-setting", "remote-control", "pin", pin)
  end
  local args = { exe, "--ipc", ipc_path(), "--port", tostring(o.port), "--pin", pin }
  local me
  me = mp.command_native_async({ name = "subprocess", args = args, playback_only = false }, function(_, r)
    if job == me then job = nil end
    if r and r.status and r.status ~= 0 and not r.killed_by_us then
      mp.msg.warn("vivi-remote exited with " .. tostring(r.status) .. " " .. tostring(r.error_string or ""))
    end
  end)
  job = me
end

options.read_options(o, "remote-control", function()
  -- Our own vivi-setting for a generated PIN comes back here: no restart needed.
  if o.port ~= last.port or o.pin ~= last.pin then
    start()
  end
end)
mp.register_script_message("address", function()
  toast("Remote control: " .. mp.get_property("user-data/remote/url", job and "starting…" or "not running"))
end)
mp.register_event("shutdown", stop)
start()
