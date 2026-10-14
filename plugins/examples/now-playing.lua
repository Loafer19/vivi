-- @name Now Playing OSD
-- @description Shows the title on mpv's OSD for three seconds when a file starts
-- @version 1.0
-- @author Vivi
--
-- Example external plugin. Copy it to ~/.config/vivi/scripts/ and enable it in
-- Settings → Extensions. Any mpv Lua or JavaScript script works the same way.

mp.register_event("file-loaded", function()
    local title = mp.get_property("media-title", "")
    mp.msg.info("now playing: " .. title)
    mp.osd_message(title, 3)
end)
