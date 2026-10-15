-- @name Discord Rich Presence
-- @version 1.0.0
-- @author Vivi
-- @description Discord status from what Vivi plays (Vivi plugin package).
--
-- The work is done by the native helper bin/vivi-discord (installed with the package for
-- this platform): this script makes sure mpv's JSON IPC server (input-ipc-server) is on,
-- starts the helper with the socket path and the settings, restarts it when a setting
-- changes and stops it on shutdown. The helper polls mpv over IPC, talks to Discord's
-- local IPC socket / named pipe and reports its state in user-data/discord/status.
local mp = require "mp"
local options = require "mp.options"

local o = { ["app-id"] = "", ["hide-names"] = false, image = "", pkgdir = "" }
local windows = package.config:sub(1, 1) == "\\"
local job

local function toast(msg)
  mp.commandv("script-message", "vivi-toast", msg)
end

-- mpv's IPC endpoint; turned on here when nobody set one.
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

local function stop()
  if job then
    mp.abort_async_command(job)
    job = nil
  end
end

local function start()
  stop()
  local exe = helper("vivi-discord")
  if not exe then
    toast("Discord Rich Presence: helper not found, reinstall the plugin")
    return
  end
  local args = { exe, "--ipc", ipc_path(), "--app-id", o["app-id"], "--image", o.image }
  if o["hide-names"] then args[#args + 1] = "--hide-names" end
  local me
  me = mp.command_native_async({ name = "subprocess", args = args, playback_only = false }, function(ok, r)
    if job == me then job = nil end
    if r and r.status and r.status ~= 0 and not r.killed_by_us then
      mp.msg.warn("vivi-discord exited with " .. tostring(r.status) .. " " .. tostring(r.error_string or ""))
    end
  end)
  job = me
end

options.read_options(o, "discord", start)
mp.register_script_message("status", function()
  toast(mp.get_property("user-data/discord/status", job and "Starting…" or "Not running"))
end)
mp.register_event("shutdown", stop)
start()
