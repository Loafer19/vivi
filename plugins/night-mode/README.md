# Night Mode

Compresses loud scenes and lifts quiet dialogue so films stay understandable at low volume (mpv dynaudnorm / loudnorm). Toggle with Alt+N.

Install: Vivi → Settings → Extensions → Browse → **Install**. Requires Vivi 0.8.0+. Implements #60.

- `Alt+N`: toggle for this session
- Settings: *Night mode on* (state at startup, applied live) and *Strength* (Light / Medium / Strong `dynaudnorm`, or EBU R128 `loudnorm`).

The filter is added as `@vivinight` to mpv's `af` chain.
