//! Control-socket commands: bookmarks.

use super::*;
use crate::player::Player;
use crate::AppWindow;

pub(super) fn bookmarks(_a: &AppWindow, p: &Player, args: &Value) -> Result<Value, String> {
    // Bookmarks of the playing file; {"add": true} bookmarks the position first.
    if args["add"].as_bool() == Some(true) {
        crate::bookmarks::add_current(p, false);
    }
    Ok(json!(crate::bookmarks::current_marks()))
}

pub(super) fn bookmark_add(_a: &AppWindow, p: &Player, args: &Value) -> Result<Value, String> {
    // Bookmark the playing position: {"label"?, "note"?}.
    let s = |k: &str| args[k].as_str().unwrap_or("").to_string();
    match crate::bookmarks::add_here(p, &s("label"), &s("note"), false) {
        Ok(i) => {
            Ok(json!({ "added": true, "index": i, "bookmarks": crate::bookmarks::current_marks() }))
        }
        Err(usize::MAX) => Err("nothing is playing".into()),
        Err(i) => Ok(
            json!({ "added": false, "index": i, "bookmarks": crate::bookmarks::current_marks() }),
        ),
    }
}

/// `{"index": n}` of the playing file's bookmarks (see "bookmarks").
fn mark_index(args: &Value) -> Result<usize, String> {
    let i = args["index"]
        .as_u64()
        .ok_or("missing \"index\" (0-based, see list_bookmarks)")? as usize;
    let n = crate::bookmarks::current_marks().len();
    if i >= n {
        return Err(format!("no bookmark {i}: this file has {n}"));
    }
    Ok(i)
}

pub(super) fn bookmark_remove(_a: &AppWindow, _p: &Player, args: &Value) -> Result<Value, String> {
    crate::bookmarks::remove_current(mark_index(args)?);
    Ok(json!(crate::bookmarks::current_marks()))
}

pub(super) fn bookmark_jump(_a: &AppWindow, p: &Player, args: &Value) -> Result<Value, String> {
    let i = mark_index(args)?;
    crate::bookmarks::jump(p, i);
    let t = crate::bookmarks::current_marks()[i].time;
    Ok(json!({ "time": t }))
}

pub(super) fn bookmarks_export(a: &AppWindow, p: &Player, args: &Value) -> Result<Value, String> {
    // {"to": "player"} merges the bookmarks into the chapter list (until the file
    // closes); {"to": "file", "path": "/x.ffmetadata"} writes an FFmpeg metadata
    // chapters file (default: <video>.ffmetadata next to a local video).
    match args["to"].as_str().unwrap_or("player") {
        "player" => {
            let added = crate::bookmarks::to_chapters(p)?;
            Ok(json!({ "added": added, "chapters": p.chapters().len() }))
        }
        "file" => {
            let path = match args["path"].as_str() {
                Some(s) if !s.trim().is_empty() => std::path::PathBuf::from(s.trim()),
                _ => {
                    let cur = p.get("path").unwrap_or_default();
                    if !std::path::Path::new(&cur).is_file() {
                        return Err("give a \"path\" (the playing file is not a local file)".into());
                    }
                    crate::bookmarks::chapters_file_for(&cur)
                }
            };
            let f = crate::bookmarks::to_chapters_file(p, Some(path))?;
            Ok(json!({ "path": f.to_string_lossy() }))
        }
        t => Err(format!("unknown \"to\" {t:?} (player or file)")),
    }
}
