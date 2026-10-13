//! Seek-bar hover preview (#10): evenly spaced frames rendered in the background with the
//! `ffmpeg` CLI (fast keyframe seeks, small RGBA frames), kept in memory for the current file.

use crate::AppWindow;
use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub const FRAMES: usize = 50;
const W: u32 = 192;
const H: u32 = 108;

/// Bumped on every new file so stale workers stop early.
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn frame(path: &str, t: f64) -> Option<Vec<u8>> {
    let vf = format!(
        "scale={W}:{H}:force_original_aspect_ratio=decrease,pad={W}:{H}:(ow-iw)/2:(oh-ih)/2"
    );
    let out = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-nostdin",
            "-skip_frame",
            "nokey",
            "-ss",
            &format!("{t:.2}"),
            "-i",
            path,
        ])
        .args([
            "-an",
            "-sn",
            "-frames:v",
            "1",
            "-vf",
            &vf,
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-",
        ])
        .output()
        .ok()?;
    (out.stdout.len() == (W * H * 4) as usize).then_some(out.stdout)
}

/// Start generating previews for `path` (local video files only).
pub fn start(app: &AppWindow, path: &str, duration: f64, enabled: bool) {
    let gen = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let model: Rc<VecModel<Image>> = Rc::new(VecModel::default());
    app.set_preview_frames(ModelRc::from(model.clone()));
    let p = Path::new(path);
    if !enabled || duration <= 1.0 || !p.is_file() || crate::library::kind_of(p) != Some("video") {
        return;
    }
    // Placeholder frames are filled in as they arrive (index order), so the
    // preview works for the parts that are ready.
    let path = Arc::new(path.to_string());
    let w = app.as_weak();
    let threads = 3;
    for k in 0..threads {
        let path = path.clone();
        let w = w.clone();
        std::thread::spawn(move || {
            for i in (k..FRAMES).step_by(threads) {
                if GENERATION.load(Ordering::SeqCst) != gen {
                    return;
                }
                let t = duration * (i as f64 + 0.5) / FRAMES as f64;
                let Some(px) = frame(&path, t) else { continue };
                let w = w.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if GENERATION.load(Ordering::SeqCst) != gen {
                        return;
                    }
                    let Some(a) = w.upgrade() else { return };
                    let img = Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                        &px, W, H,
                    ));
                    PENDING.with(|p| {
                        let mut p = p.borrow_mut();
                        if p.len() != FRAMES {
                            *p = vec![None; FRAMES];
                        }
                        p[i] = Some(img);
                        // Publish once every slot has some frame: missing slots borrow the nearest ready one.
                        let ready = p.iter().filter(|x| x.is_some()).count();
                        if ready % 10 == 0 || ready == FRAMES {
                            let filled: Vec<Image> = (0..FRAMES)
                                .filter_map(|j| {
                                    (0..FRAMES)
                                        .flat_map(|d| [j.checked_sub(d), Some(j + d)])
                                        .flatten()
                                        .find_map(|x| p.get(x).cloned().flatten())
                                })
                                .collect();
                            a.set_preview_frames(ModelRc::new(VecModel::from(filled)));
                        }
                    });
                });
            }
        });
    }
    PENDING.with(|p| p.borrow_mut().clear());
}

thread_local! {
    static PENDING: std::cell::RefCell<Vec<Option<Image>>> = const { std::cell::RefCell::new(Vec::new()) };
}
