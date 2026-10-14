# A-B Loop & Frame Step

Set and clear A-B loop points from the keyboard, nudge them by a frame, step frame by frame while paused and see the loop on an on-screen bar.

Install: Vivi → Settings → Extensions → Browse → **Install**. Requires Vivi 0.8.0+. Implements #59.

- `Alt+A` / `Alt+B`: set A / B at the current position
- `Alt+C`: clear the loop
- `Alt+Left` / `Alt+Right`: move the last set point by one frame
- `,` / `.`: one frame back / forward while paused (1 s seek while playing)

The loop bar is an mpv OSD overlay above the controls. Binding names for `script-binding ab_loop/<name>` (`set-a`, `set-b`, `clear`, `nudge-back`, `nudge-forward`, `frame-back`, `frame-forward`).
