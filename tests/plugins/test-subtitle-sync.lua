local T = dofile("tests/plugins/stub.lua")
-- run.sh points XDG_DATA_HOME at a temporary folder (offsets.json is written there).
local P = T.load("plugins/subtitle-sync/subtitle-sync.lua")
T.eq(P.out_path("/m/movie.en.srt"), "/m/movie.en.synced.srt", "synced name")
T.eq(P.out_path("/m/movie.en.synced.srt"), "/m/movie.en.synced.srt", "no double suffix")
T.eq(P.out_path("/m/noext"), "/m/noext.synced.srt", "no extension")
T.eq(P.round(0.30000000004), 0.3, "round")
-- Nudges go through sub-delay and are saved per file.
T.props.path = "tests/plugins/stub.lua"
T.props["sub-delay"] = 0
P.nudge(1)()
P.nudge(1)()
T.eq(T.props["sub-delay"], 0.2, "two steps later")
P.nudge(-10)()
T.eq(T.props["sub-delay"], -0.8, "one second earlier")
T.ok(P.OFFSETS:find("subtitle-sync", 1, true), "offsets file path")
T.done("subtitle-sync")
