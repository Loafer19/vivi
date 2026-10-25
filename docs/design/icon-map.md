# Icon map

Icons in `assets/icons/` sliced from `docs/design/frames/icons-sheet.svg` (Figma export, 548×812, 24 px cells on a 44 px grid starting at 20,20).
Cell `rRRcCC` = row RR, column CC (0-based). Each file keeps the original strokes/fills (#8F8F8F text, #D37918 accent, #AB6314 dark accent) with the viewBox moved onto its cell.
Matching: both sets rendered with rsvg, compared by blurred-mask cosine similarity (plus accent-mask similarity); scores ≥ 0.95 accepted, the rest picked by eye.
Slint `Icon` (colorize) recolours the single-tone uses (glyphs of about 16 px and less); icons drawn at about
18 px and up are two-tone (`TriIcon`), see [architecture → Icon conventions](../architecture.md#icon-conventions).

## Two-tone layers and names

`python3 tools/icon_layers.py` splits every icon with accent parts into `assets/icons/layers/<name>-{b,a,d}.svg`
(text colour, `#D37918` accent, `#AB6314` dim accent) and writes `ui/core/duo.slint`, the `Duo` lookup that
`TriIcon { name: "…" }` and `IconButton { duo: "…" }` use. A name resolves to `<name>-twotone.svg` when that
file mixes text-coloured and accent parts, else to `<name>.svg`'s own layers, else to the plain `<name>.svg`
(single-tone). All-accent art (`*-active`, menu rows) is never picked for a plain name; `<name>-active` is the Duo name of the
solid active art.

Two-tone rule used for the files marked #82: the outline stays in the text colour and one secondary part
(the note, the plus, the needle, the arrow head, the second bar) turns dim accent; glyphs that are a single
shape get a dim fill: play and stop keep their solid shape (a 2 px outline over a dim fill of the same
path, so a primary button that draws both tones in one colour still shows the solid glyph), the pointer
gets a smaller dim fill inside its outline. Left single-tone: `eq` and `remove` (a single stroke has no second part, and an
all-accent stroke reads as an active state). `eq-*` presets are drawn at 16 px in the preset list, so they
stay single-tone there.

Player bars: Settings → General → Appearance → **Player bar icons** (`interface.bar_icons`, *Two-tone* by
default, or *Single-tone*) sets `Theme.bar-duo`. Buttons of the player's top bar (open, library, miniplayer,
settings, fullscreen, close) and bottom bar (play / pause, previous / next, ±10 s, volume, the toolbar
items, plugin buttons, ⋯) are `IconButton { bar: true }`; with Single-tone their `TriIcon` is `flat` and
draws `Duo.single(name)`, the plain `<name>.svg` (or every layer in the text colour when a name has no
plain file). Everything else stays two-tone, and 16 px icons are single-tone either way.

Active states: a selected navigation entry (library rail, Settings categories) draws the solid all-accent
`<name>-active` art (filled shape, or 2 px accent strokes for line icons like Recent and Downloads), the
inactive one the two-tone art. Player-bar toggles and side-panel tabs tint the icon accent when on (tabs also
get an underline); menu rows have no active state (checked rows show a check mark).

Comparison for #82: `docs/design/compare/icons-two-tone.jpg` (before | after, dark and light theme).

Comparison: `docs/design/compare/icons.png` (old left, new right).

| Icon | Sheet cell |
|---|---|
| `accessibility.svg` | r10c05 |
| `add.svg` | r08c10 |
| `add-twotone.svg` | r08c10 + dim horizontal stroke (#82) |
| `album.svg` | r05c01 |
| `album-twotone.svg` | r05c01 + dim note (#82) |
| `arrow-down.svg` | r07c03 |
| `arrow-down-twotone.svg` | r07c04 |
| `arrow-down-active.svg` | r07c03, 2 px accent strokes (active Downloads rail entry) |
| `arrow-left.svg` | r07c10 |
| `arrow-left-twotone.svg` | r07c11 |
| `arrow-right.svg` | r07c08 |
| `arrow-right-twotone.svg` | r07c09 |
| `arrow-up.svg` | r07c05 |
| `arrow-up-twotone.svg` | r07c06 |
| `artist.svg` | r10c01 |
| `artist-active.svg` | r10c04 |
| `artist-twotone.svg` | r10c01 + dim note (#82) |
| `audio.svg` | r01c06 |
| `audio-active.svg` | r01c08 |
| `audio-twotone.svg` | r01c07 |
| `backward-10-sec.svg` | r02c11 |
| `backward-10-sec-twotone.svg` | r02c11 + dim "10" (#82) |
| `backward-15-sec.svg` | r03c00 |
| `bookmark.svg` | r13c01 |
| `bookmark-twotone.svg` | r13c01 + dim plus (#82) |
| `bookmarks.svg` | r12c11 |
| `bookmarks-twotone.svg` | r13c00 |
| `capture-device.svg` | r16c02 |
| `capture-device-twotone.svg` | r16c03 |
| `chapters.svg` | r12c09 |
| `chapters-twotone.svg` | r12c10 |
| `check-active.svg` | r09c06 |
| `check-selected.svg` | r09c05 |
| `check-selected-twotone.svg` | r09c07 |
| `chevron-down.svg` | r07c02 |
| `chevron-left.svg` | r08c07 |
| `chevron-right.svg` | r08c06 |
| `chevron-up.svg` | r08c05 |
| `close.svg` | r03c11 |
| `close-low.svg` | r03c08 |
| `close-low-twotone.svg` | r03c08 + dim wave (#82) |
| `close-medium.svg` | r03c07 |
| `close-medium-twotone.svg` | r03c07 + dim waves (#82) |
| `close-none.svg` | r03c09 |
| `close-none-twotone.svg` | r03c09 + dim × (#82) |
| `close-window.svg` | r03c11 at 10 px, the top-bar close button in Single-tone |
| `close-window-twotone.svg` | dim circle around a 6 px × (top-bar close button) |
| `collapse-sidebar.svg` | r05c04 |
| `disc.svg` | r16c00 |
| `disc-twotone.svg` | r16c01 |
| `divider.svg` | r07c01 |
| `edit.svg` | r15c09 |
| `edit-twotone.svg` | r15c09 + dim baseline (#82) |
| `eq.svg` | r13c08 |
| `eq-classical.svg` | r13c09 |
| `eq-club.svg` | r13c10 |
| `eq-dance.svg` | r13c11 |
| `eq-full_bass.svg` | r14c00 |
| `eq-full_bass_and_treble.svg` | r14c01 |
| `eq-full_treble.svg` | r14c02 |
| `eq-headphones.svg` | r14c03 |
| `eq-large_hall.svg` | r14c09 |
| `eq-live.svg` | r15c01 |
| `eq-party.svg` | r15c00 |
| `eq-pop.svg` | r14c11 |
| `eq-reggae.svg` | r14c10 |
| `eq-rock.svg` | r14c04 |
| `eq-ska.svg` | r14c05 |
| `eq-soft.svg` | r14c06 |
| `eq-soft_rock.svg` | r14c08 |
| `eq-techno.svg` | r14c07 |
| `expand-sidebar.svg` | r05c03 |
| `favorite-active.svg` | r15c08 |
| `favorite-twotone.svg` | r15c07 outline in text colour + smaller dim heart inside (#82; the sheet cell was all dim accent) |
| `file.svg` | r15c10 |
| `file-twotone.svg` | r15c10 + dim text lines (#82) |
| `folder.svg` | r04c05 |
| `folder-active.svg` | r04c09 |
| `folder-selected.svg` | r04c10 |
| `folder-twotone.svg` | r04c05 + dim front-panel line (#82; the sheet cell r04c06 was mostly accent) |
| `forward-10-sec.svg` | r02c09 |
| `forward-10-sec-twotone.svg` | r02c09 + dim "10" (#82) |
| `forward-15-sec.svg` | r02c10 |
| `fullscreen.svg` | r04c02 |
| `fullscreen-active.svg` | r04c03 |
| `fullscreen-twotone.svg` | r04c02, top-right / bottom-left corners dim (#82) |
| `fullscreen-active-twotone.svg` | r04c03, top-right / bottom-left corners dim (#82) |
| `genre.svg` | r17c02 |
| `genre-twotone.svg` | r17c03 |
| `home.svg` | r04c04 |
| `home-active.svg` | r06c11 |
| `home-selected.svg` | r06c11 |
| `home-twotone.svg` | r04c04 + dim door (#82) |
| `hotkeys.svg` | r01c00 |
| `hotkeys-active.svg` | r01c02 |
| `hotkeys-twotone.svg` | r01c01 |
| `info.svg` | r10c06 |
| `info-twotone.svg` | r10c07 |
| `info-active.svg` | r10c06, solid accent disc with the i cut out |
| `interface.svg` | r10c08 |
| `interface-active.svg` | r10c08, pointer filled accent |
| `interface-twotone.svg` | r10c08 + dim fill inside the outline (#82) |
| `link.svg` | r09c02 |
| `link-active.svg` | r09c03 |
| `link-twotone.svg` | r09c02, right half dim (#82; the sheet cell r09c03 is the linked art, see link-active) |
| `list.svg` | r11c11 |
| `lists.svg` | r00c10 |
| `lists-active.svg` | r00c11 |
| `maximize.svg` | r04c01 |
| `minimize.svg` | r04c00 |
| `more.svg` | r08c08 |
| `more-twotone.svg` | r08c08, middle dot dim (#82) |
| `move.svg` | r03c10 |
| `music.svg` | r05c06 |
| `music-twotone.svg` | r05c06, note head dim (#82) |
| `new_folder.svg` | r04c07 |
| `new_folder-twotone.svg` | r04c08 |
| `next.svg` | r02c03 |
| `next-twotone.svg` | r02c03, bar dim (#82; the sheet cell r02c04 was all dim accent) |
| `open.svg` | r08c00 |
| `open-twotone.svg` | r08c00, arrow dim (#82) |
| `paint.svg` | r11c07 |
| `paint-active.svg` | r11c09 |
| `paint-twotone.svg` | r11c08 |
| `pause.svg` | r02c02 |
| `pause-twotone.svg` | r02c02, right bar dim (#82) |
| `pip.svg` | r13c06 |
| `pip-twotone.svg` | r13c06, inner window dim (#82) |
| `play.svg` | r01c09 |
| `play-twotone.svg` | r01c09 outline (2 px) over a dim fill of the same shape, solid on primary buttons (#82) |
| `playback.svg` | r01c10 |
| `playback-active.svg` | r01c11 outline, filled accent |
| `playback-twotone.svg` | r01c11 |
| `playback-selected-twotone.svg` | r01c11 |
| `playback_settings.svg` | r00c02 |
| `playback_settings-active.svg` | `settings-active.svg` (solid gear, play cut out) |
| `playback_settings-twotone.svg` | r00c02, play triangle dim (#82) |
| `playlist.svg` | r00c07 |
| `playlist-active.svg` | r00c09 |
| `playlist-twotone.svg` | r00c08 |
| `plugins.svg` | r16c04 |
| `plugins-active.svg` | r16c06 |
| `plugins-twotone.svg` | r16c05 |
| `preferences.svg` | r00c05 |
| `preferences-active.svg` | r00c06 |
| `preferences-twotone.svg` | r00c05, slider knobs dim (#82) |
| `previous.svg` | r02c07 |
| `previous-twotone.svg` | r02c07, bar dim (#82; the sheet cell r02c08 was all dim accent) |
| `privacy.svg` | r16c07 |
| `privacy-active.svg` | r16c09 |
| `privacy-twotone.svg` | r16c08 |
| `profile.svg` | r10c02 |
| `profile-twotone.svg` | r10c03 |
| `queue.svg` | r12c07 |
| `queue-twotone.svg` | r12c08 |
| `quit.svg` | r16c10 |
| `quit-twotone.svg` | r16c11 |
| `recent.svg` | r03c04 |
| `recent-twotone.svg` | r03c05 |
| `recent-active.svg` | r03c04, 2 px accent strokes (active Recent rail entry) |
| `record.svg` | r12c03 |
| `record-active.svg` | r12c04 |
| `record-twotone.svg` | r12c04, ring in the text colour, dot dim |
| `refresh.svg` | r03c01 |
| `refresh-twotone.svg` | r03c02 |
| `remove.svg` | r08c11 |
| `repeat-right.svg` | r08c01 |
| `repeat-right-twotone.svg` | r08c02 |
| `replay.svg` | r03c03 |
| `replay-twotone.svg` | r03c03, arrow head dim (#82) |
| `save.svg` | hand-drawn in the sheet style (see compare/new-icons.png) |
| `save-twotone.svg` | r15c11 |
| `scope-twotone.svg` | hand-drawn in the sheet style (see compare/new-icons.png) |
| `search.svg` | r09c11 |
| `search-twotone.svg` | r09c11, handle dim (#82) |
| `settings.svg` | r00c00 |
| `settings-active.svg` | r00c03 |
| `settings-twotone.svg` | r00c01 |
| `shuffle-right.svg` | r08c03 |
| `shuffle-right-twotone.svg` | r08c04 |
| `sidebar-active.svg` | r05c05 |
| `sidebar-left.svg` | r04c11 |
| `sidebar-right.svg` | r05c00 |
| `snapshot.svg` | r12c06 |
| `snapshot-twotone.svg` | r12c06, lens dim (#82) |
| `songs.svg` | r05c10 |
| `songs-active.svg` | r05c11 |
| `songs-selected.svg` | r05c11 |
| `songs-twotone.svg` | r05c10, note dim (#82) |
| `sort.svg` | r09c09 |
| `sort-active.svg` | r09c10, down arrow dim to match sort-twotone (#82) |
| `sort-twotone.svg` | r09c09, down arrow dim (#82) |
| `spectometer-twotone.svg` | hand-drawn in the sheet style (see compare/new-icons.png) |
| `spectrum-twotone.svg` | hand-drawn in the sheet style (see compare/new-icons.png) |
| `speed.svg` | r12c01 |
| `speed-active.svg` | r12c02 |
| `speed-twotone.svg` | r12c01, needle dim (#82) |
| `stop.svg` | r02c05 |
| `stop-twotone.svg` | r02c05 outline (2 px) over a dim fill of the same shape (#82; the sheet cell r02c06 was all dim accent) |
| `streaming.svg` | r06c04 |
| `streaming-active.svg` | r06c06 |
| `streaming-selected.svg` | r06c06 |
| `streaming-twotone.svg` | r06c05 |
| `subtitle.svg` | r10c10 |
| `subtitle-twotone.svg` | r10c10, text lines dim (#82) |
| `subtitles.svg` | r01c03 |
| `subtitles-active.svg` | r01c05 |
| `subtitles-twotone.svg` | r01c04 |
| `swap.svg` | r11c02 |
| `swap-twotone.svg` | r11c03 |
| `thumbnail.svg` | r15c02 |
| `thumbnail-high.svg` | r15c03 |
| `thumbnail-low.svg` | r15c05 |
| `thumbnail-medium.svg` | r15c04 |
| `tv.svg` | r06c08 |
| `tv-active.svg` | r06c09 |
| `tv-selected.svg` | r06c09 |
| `ui.svg` | r17c00 |
| `ui-twotone.svg` | r17c01 |
| `upload-up-twotone.svg` | r07c07 |
| `video.svg` | hand-drawn in the sheet style (see compare/new-icons.png) |
| `video-active.svg` | r05c09 |
| `video-twotone.svg` | r05c08 |
| `videos.svg` | r06c01 |
| `videos-active.svg` | r06c02 |
| `videos-selected.svg` | r06c02 |
| `videos-twotone.svg` | r06c01, play triangle dim (#82) |
| `view.svg` | r09c00 |
| `view-active.svg` | r09c01 |
| `volume-high.svg` | r03c06 |
| `volume-high-twotone.svg` | r03c06, waves dim (#82) |
| `waveform-twotone.svg` | r01c07 |
| `window.svg` | r11c05 |
| `window-twotone.svg` | hand-drawn in the sheet style (see compare/new-icons.png) |
| `wrench.svg` | r11c06 |
| `zoom.svg` | r10c00 |
| `zoom-twotone.svg` | r10c00, plus dim (#82) |

Menu icons (`assets/icons/menu/`) come from the same sheet where the Figma menu shapes were stroke-outlined: record (r12c04), hide-ui (r17c00), video (r05c08), subtitle / add & edit subtitle tracks (r01c04), open-media (r11c01), add-to-queue (r12c08), list-view (r11c11), grid-view (r11c10), artist (r10c01).
