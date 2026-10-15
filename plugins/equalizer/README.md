# Equalizer

Presets and a custom 10-band equalizer applied with mpv audio filters. Implements #74.

| Preset | 31 | 62 | 125 | 250 | 500 | 1k | 2k | 4k | 8k | 16k |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Flat | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| Bass | +6 | +5 | +4 | +2 | 0 | 0 | 0 | 0 | 0 | 0 |
| Vocal | −2 | −2 | −1 | +1 | +3 | +4 | +4 | +2 | 0 | −1 |
| Rock | +5 | +3 | +1 | −1 | −2 | −1 | +1 | +3 | +4 | +5 |
| Night | −6 | −5 | −3 | −1 | +1 | +2 | +2 | +1 | 0 | −1 |
| Custom | your ten values (dB, −12…12) | | | | | | | | | |

* `Alt+E` cycles the presets, `Alt+Shift+E` turns the equalizer off and on. Both are saved
  as plugin settings, so the choice survives restarts.
* *Filter*: `equalizer` = ten ffmpeg peaking filters, one octave wide; `superequalizer` =
  ffmpeg's 18-band FIR equalizer with the ten gains interpolated over frequency.
* *Lower the volume by the largest boost*: a pre-amp (`volume=-<max>dB`) so boosted
  presets do not clip.
* The filter is labelled `@vivi-eqplug`, separate from the Audio panel's EQ
  (`@vivi-eq`); if both are on, their effects add up. Night pairs well with the Night
  Mode plugin (dynamic range compression).

No network, no files, no helpers.
