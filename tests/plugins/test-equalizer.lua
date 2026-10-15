local T = dofile("tests/plugins/stub.lua")
local P = T.load("plugins/equalizer/equalizer.lua")
local g = P.parse_gains("3, -2 20 x 1.5")
T.eq(g[1], 3, "gain 1")
T.eq(g[3], 12, "clamped")
T.eq(g[4], 1.5, "decimal")
T.eq(g[10], 0, "missing bands are 0")
T.eq(P.graph(P.PRESETS.Flat, "equalizer (10 bands)", true), nil, "flat = no filter")
T.eq(P.graph(P.PRESETS.Bass, "equalizer (10 bands)", true),
  "equalizer=f=31:t=o:w=1:g=6,equalizer=f=62:t=o:w=1:g=5,equalizer=f=125:t=o:w=1:g=4,equalizer=f=250:t=o:w=1:g=2,volume=-6dB",
  "bass graph")
T.eq(P.graph(P.PRESETS.Night, "equalizer (10 bands)", false):find("volume"), nil, "no preamp")
local s = P.graph(P.PRESETS.Rock, "superequalizer (18 bands)", false)
T.ok(s:find("^superequalizer=1b=1.39:"), "super band 1-2 interpolated")
T.ok(s:find(":18b=1.78$"), "super band 18 (+5 dB)")
T.eq(P.gain_at(P.PRESETS.Rock, 1000), -1, "interp at a band")
T.eq(math.floor(P.gain_at({ 0, 0, 0, 0, 0, 0, 6, 0, 0, 0 }, 2828) * 100 + 0.5), 300, "interp between bands")
-- applying adds a labelled filter; cycling saves the preset
P.next_preset()
local last = T.calls[#T.calls]
local added
for _, c in ipairs(T.calls) do if c[1] == "af" and c[2] == "add" then added = c[3] end end
T.ok(added and added:find("^@vivi%-eqplug:lavfi=%[equalizer=f=31"), "af add with label")
T.eq(P.o.preset, "Bass", "next preset")
T.done("equalizer")
