local T = dofile("tests/plugins/stub.lua")
local P = T.load("plugins/cut-export/cut-export.lua")
T.eq(P.parse_time("1:02:03.5"), 3723.5, "h:m:s")
T.eq(P.parse_time("62:03"), 3723, "m:s")
T.eq(P.parse_time("12.25"), 12.25, "seconds")
T.eq(P.parse_time(""), nil, "empty")
T.eq(P.parse_time("x:1"), nil, "invalid")
T.eq(P.fmt_time(62.5), "1:02.500", "fmt")
T.eq(P.fmt_time(3723.5), "1:02:03.500", "fmt hours")
-- Range: In/Out, then A-B loop, then the whole file; swapped points are ordered.
T.props.duration = 100
T.props["ab-loop-a"], T.props["ab-loop-b"] = 10, 20
local a, b, src = P.range()
T.eq(a, 10, "ab a") T.eq(b, 20, "ab b") T.eq(src, "A-B loop", "ab source")
P.o["in"], P.o.out = "0:50", "0:40"
a, b, src = P.range()
T.eq(a, 40, "swapped a") T.eq(b, 50, "swapped b") T.eq(src, "In/Out", "explicit")
P.o["in"], P.o.out = "5", ""
a, b = P.range()
T.eq(b, 100, "out defaults to duration")
T.eq(P.out_path("/v/movie.mkv", 62.5, 70, "", false, false), "/v/movie [1m02.5s-1m10.0s].mkv", "copy name")
T.eq(P.out_path("/v/song.flac", 0, 30, "/out", true, true), "/out/song [0m00.0s-0m30.0s].m4a", "re-encode audio")
local args = table.concat(P.ffmpeg_args("ffmpeg", "in.mkv", 1, 3.5, "o.mkv", false), " ")
T.ok(args:find("-ss 1.000 -i in.mkv -t 2.500 -map 0 -c copy", 1, true), "copy args")
args = table.concat(P.ffmpeg_args("ffmpeg", "in.mkv", 1, 3.5, "o.mp4", true, false), " ")
T.ok(args:find("-c:v libx264", 1, true), "x264 args")
args = table.concat(P.ffmpeg_args("ffmpeg", "in.flac", 1, 3.5, "o.m4a", true, true), " ")
T.ok(args:find("-vn -c:a aac", 1, true), "audio args")
-- Same pinned media ffmpeg build as Subtitle Auto-Sync (no other ffmpeg flavor).
local f = io.open("plugins/subtitle-sync/subtitle-sync.lua")
local ss = f:read("*a")
f:close()
T.ok(ss:find(P.FF_MEDIA.tag, 1, true), "same ffmpeg tag")
for k, v in pairs(P.FF_MEDIA.sha) do T.ok(ss:find(k .. "\"] = \"" .. v, 1, true), "same sha " .. k) end
T.done("cut-export")
