//! Video without OpenGL: mpv's software render API (`MPV_RENDER_API_TYPE_SW`) draws each
//! frame into memory, shown as an image under the UI (`Playback.sw-frame`). Used when the
//! window runs on Slint's software renderer (no OpenGL 2 driver, e.g. Windows in a VM
//! without GPU passthrough, or `VIVI_RENDERER=software`). Scaling and colour conversion run
//! on the CPU, so large windows are capped at [`MAX_PIXELS`] and Slint scales up the rest.

use libmpv2::Mpv;
use libmpv2_sys as sys;
use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Largest frame rendered (1080p); bigger windows get a scaled-down frame.
const MAX_PIXELS: u32 = 1920 * 1080;

struct Ctx {
    raw: *mut sys::mpv_render_context,
    /// Keeps the mpv core alive while the render context exists.
    _mpv: Arc<Mpv>,
}

thread_local! {
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
    static WINDOW: RefCell<Option<slint::Weak<crate::AppWindow>>> = const { RefCell::new(None) };
    /// Size of the last rendered frame, to redraw a paused frame after a resize.
    static LAST: Cell<(u32, u32)> = const { Cell::new((0, 0)) };
    static RESIZE: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

/// A frame request is queued on the event loop (mpv can call back much faster).
static QUEUED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn on_update(_: *mut c_void) {
    if !QUEUED.swap(true, Ordering::AcqRel) {
        let _ = slint::invoke_from_event_loop(|| draw(false));
    }
}

/// Create the software render context and mark rendering ready. Without one (old libmpv)
/// video stays off; audio still plays.
pub(super) fn attach(app: &crate::AppWindow, mpv: Arc<Mpv>) {
    WINDOW.with(|w| *w.borrow_mut() = Some(app.as_weak()));
    // "Fast" scaling: the software path must keep up with playback on a CPU.
    let _ = mpv.set_property("sw-fast", "yes");
    let mut params = [
        sys::mpv_render_param {
            type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
            data: sys::MPV_RENDER_API_TYPE_SW.as_ptr() as *mut c_void,
        },
        sys::mpv_render_param {
            type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
            data: std::ptr::null_mut(),
        },
    ];
    let mut raw: *mut sys::mpv_render_context = std::ptr::null_mut();
    // SAFETY: the mpv handle is valid (kept alive by `Ctx::_mpv`); params end with INVALID.
    let err =
        unsafe { sys::mpv_render_context_create(&mut raw, mpv.ctx.as_ptr(), params.as_mut_ptr()) };
    if err < 0 || raw.is_null() {
        crate::app::fatal::note(&format!(
            "mpv software render context failed ({err}); video output is off, audio plays"
        ));
        show_notice(
            app,
            "No OpenGL driver: video output is off, audio still plays",
        );
        return;
    }
    // SAFETY: `raw` is a live render context; the callback only touches statics.
    unsafe {
        sys::mpv_render_context_set_update_callback(raw, Some(on_update), std::ptr::null_mut())
    };
    CTX.with(|c| *c.borrow_mut() = Some(Ctx { raw, _mpv: mpv }));
    app.global::<crate::Playback>().set_sw_video(true);
    super::READY.store(true, Ordering::Release);
    // A paused frame is redrawn at the new size after the window is resized.
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(250),
        || {
            if target_size().is_some_and(|s| s != LAST.with(Cell::get)) {
                draw(true);
            }
        },
    );
    RESIZE.with(|r| *r.borrow_mut() = Some(timer));
    show_notice(
        app,
        "No OpenGL driver: software rendering, video may be slow",
    );
}

fn show_notice(app: &crate::AppWindow, msg: &'static str) {
    let w = app.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_secs(1), move || {
        if let Some(a) = w.upgrade() {
            crate::plugins::toast(&a, msg);
        }
    });
}

/// Frame size for the current window (physical pixels, capped), `None` while nothing plays.
fn target_size() -> Option<(u32, u32)> {
    let a = WINDOW.with(|w| w.borrow().as_ref().and_then(|w| w.upgrade()))?;
    if !a.global::<crate::Playback>().get_has_media() {
        return None;
    }
    let s = a.window().size();
    let (mut w, mut h) = (s.width.max(1), s.height.max(1));
    if w * h > MAX_PIXELS {
        let k = (MAX_PIXELS as f64 / (w as f64 * h as f64)).sqrt();
        w = ((w as f64 * k) as u32).max(1);
        h = ((h as f64 * k) as u32).max(1);
    }
    Some((w, h))
}

/// Render the current frame when mpv has a new one (or always, with `force`).
fn draw(force: bool) {
    QUEUED.store(false, Ordering::Release);
    CTX.with(|c| {
        let c = c.borrow();
        let Some(ctx) = c.as_ref() else { return };
        // SAFETY: live render context, called on the thread that owns it.
        let flags = unsafe { sys::mpv_render_context_update(ctx.raw) };
        let new_frame = flags & sys::mpv_render_update_flag_MPV_RENDER_UPDATE_FRAME as u64 != 0;
        if !new_frame && !force {
            return;
        }
        let Some((w, h)) = target_size() else { return };
        let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
        let mut size = [w as i32, h as i32];
        let mut stride: usize = w as usize * 4;
        let format = c"rgb0";
        let mut params = [
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_SIZE,
                data: size.as_mut_ptr().cast(),
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_FORMAT,
                data: format.as_ptr() as *mut c_void,
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_STRIDE,
                data: (&mut stride as *mut usize).cast(),
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_POINTER,
                data: buf.make_mut_bytes().as_mut_ptr().cast(),
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];
        // SAFETY: the buffer holds `stride * h` bytes and outlives the call; every
        // parameter points at a live local of the documented type.
        let err = unsafe { sys::mpv_render_context_render(ctx.raw, params.as_mut_ptr()) };
        if err < 0 {
            return;
        }
        // "rgb0": the fourth byte is undefined; the frame is opaque.
        for p in buf.make_mut_slice() {
            p.a = 255;
        }
        LAST.with(|l| l.set((w, h)));
        if let Some(a) = WINDOW.with(|w| w.borrow().as_ref().and_then(|w| w.upgrade())) {
            a.global::<crate::Playback>()
                .set_sw_frame(slint::Image::from_rgba8(buf));
        }
    });
}

/// Free the render context (must happen before the mpv core is destroyed).
pub(super) fn release() {
    RESIZE.with(|r| r.borrow_mut().take());
    if let Some(ctx) = CTX.with(|c| c.borrow_mut().take()) {
        // SAFETY: the context is live and no longer reachable from `draw`.
        unsafe { sys::mpv_render_context_free(ctx.raw) };
    }
}
