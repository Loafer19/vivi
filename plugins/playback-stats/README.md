# Playback Statistics

Counts time watched and listened per day, with top artists and shows. Kept locally in a small TSV file; Alt+S shows a summary, export as CSV or clear any time.

Install: Vivi → Settings → Extensions → Browse → **Install**. Requires Vivi 0.8.0+. Implements #66.

- `Alt+S` or *Show summary*: totals for the last 7/30/365 days, top 5 artists/shows and a per-day bar chart on the OSD
- *Export CSV*: writes `~/vivi-stats.csv` (date, seconds, kind, artist_or_show, title)
- *Clear statistics*: deletes the data file

Data lives in `$XDG_DATA_HOME/vivi/plugin-data/playback-stats/stats.tsv` and never leaves the machine (permission: `filesystem`). Shows are grouped from `Show S01E02` file names.
