//! mpv render API (OpenGL) drawn as an underlay beneath the Slint scene.
//! Slint (femtovg) renders into the default framebuffer; in `BeforeRendering` we let mpv
//! draw the current video frame into FBO 0, then Slint paints the (transparent) UI on top.

use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};
use libmpv2::Mpv;
use slint::{ComponentHandle, GraphicsAPI, RenderingState};
use std::ffi::{c_void, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

type ProcFn<'a> = &'a dyn Fn(&std::ffi::CStr) -> *const c_void;

fn get_proc_address(ctx: &usize, name: &str) -> *mut c_void {
    // SAFETY: ctx points at a `ProcFn` that is alive for the duration of
    // `RenderContext::new` (mpv resolves all GL symbols during creation).
    let f: &ProcFn = unsafe { &*(*ctx as *const ProcFn) };
    // GL symbol names never contain NUL; a name that did is simply "not found".
    let Ok(name) = CString::new(name) else {
        return std::ptr::null_mut();
    };
    f(&name) as *mut c_void
}

/// Minimal GL state reset before mpv draws. femtovg leaves textures, programs, buffers,
/// scissor/blend state and pixel-store settings bound after uploading images (library
/// thumbnails, seek-bar previews), which made mpv's frame come out black.
struct GlReset {
    disable: extern "system" fn(u32),
    active_texture: extern "system" fn(u32),
    bind_texture: extern "system" fn(u32, u32),
    use_program: extern "system" fn(u32),
    bind_buffer: extern "system" fn(u32, u32),
    bind_vertex_array: Option<extern "system" fn(u32)>,
    bind_framebuffer: extern "system" fn(u32, u32),
    pixel_storei: extern "system" fn(u32, i32),
}

impl GlReset {
    fn load(gpa: ProcFn) -> Option<Self> {
        unsafe fn f<T: Copy>(gpa: ProcFn, name: &str) -> Option<T> {
            let c = CString::new(name).ok()?;
            let p = gpa(&c);
            // SAFETY: caller passes the correct GL function signature for `name`.
            (!p.is_null()).then(|| unsafe { std::mem::transmute_copy::<*const c_void, T>(&p) })
        }
        // SAFETY: signatures match the OpenGL (ES) 3.x specification.
        unsafe {
            Some(Self {
                disable: f(gpa, "glDisable")?,
                active_texture: f(gpa, "glActiveTexture")?,
                bind_texture: f(gpa, "glBindTexture")?,
                use_program: f(gpa, "glUseProgram")?,
                bind_buffer: f(gpa, "glBindBuffer")?,
                bind_vertex_array: f(gpa, "glBindVertexArray"),
                bind_framebuffer: f(gpa, "glBindFramebuffer")?,
                pixel_storei: f(gpa, "glPixelStorei")?,
            })
        }
    }
    fn apply(&self) {
        const SCISSOR_TEST: u32 = 0x0C11;
        const BLEND: u32 = 0x0BE2;
        const STENCIL_TEST: u32 = 0x0B90;
        const DEPTH_TEST: u32 = 0x0B71;
        const CULL_FACE: u32 = 0x0B44;
        const TEXTURE0: u32 = 0x84C0;
        const TEXTURE_2D: u32 = 0x0DE1;
        const ARRAY_BUFFER: u32 = 0x8892;
        const ELEMENT_ARRAY_BUFFER: u32 = 0x8893;
        const PIXEL_UNPACK_BUFFER: u32 = 0x88EC;
        const FRAMEBUFFER: u32 = 0x8D40;
        const UNPACK_ALIGNMENT: u32 = 0x0CF5;
        const UNPACK_ROW_LENGTH: u32 = 0x0CF2;
        const PACK_ALIGNMENT: u32 = 0x0D05;
        for cap in [SCISSOR_TEST, BLEND, STENCIL_TEST, DEPTH_TEST, CULL_FACE] {
            (self.disable)(cap);
        }
        (self.active_texture)(TEXTURE0);
        (self.bind_texture)(TEXTURE_2D, 0);
        (self.use_program)(0);
        if let Some(b) = self.bind_vertex_array {
            b(0);
        }
        (self.bind_buffer)(ARRAY_BUFFER, 0);
        (self.bind_buffer)(ELEMENT_ARRAY_BUFFER, 0);
        (self.bind_buffer)(PIXEL_UNPACK_BUFFER, 0);
        (self.bind_framebuffer)(FRAMEBUFFER, 0);
        (self.pixel_storei)(UNPACK_ALIGNMENT, 4);
        (self.pixel_storei)(UNPACK_ROW_LENGTH, 0);
        (self.pixel_storei)(PACK_ALIGNMENT, 4);
    }
}

/// mpv `gpu-hwdec-interop` for the render context. libmpv's render API loads *every*
/// interop when this is left on auto (CUDA included, which loads libcuda and can wake a
/// laptop's discrete GPU while the window renders on the integrated one), so vivi picks
/// the one matching the GL driver it renders with. `config` is `[playback] hwdec_interop`
/// ("auto" or empty = pick), `hwdec` the `[playback] hwdec` setting. `None` = leave mpv's
/// default (all interops).
pub fn pick_interop(config: &str, hwdec: &str, vendor: &str, renderer: &str) -> Option<String> {
    let config = config.trim();
    if !config.is_empty() && !config.eq_ignore_ascii_case("auto") {
        return Some(config.to_string());
    }
    if !cfg!(target_os = "linux") {
        return None;
    }
    // An explicit hardware decoder needs its own interop.
    match hwdec {
        h if h.starts_with("nvdec") || h.starts_with("cuda") => return Some("cuda".into()),
        h if h.starts_with("vaapi") => return Some("vaapi".into()),
        h if h.starts_with("vulkan") || h.starts_with("drm") || h.starts_with("vdpau") => {
            return None
        }
        _ => {}
    }
    let gl = format!("{vendor} {renderer}").to_ascii_lowercase();
    if gl.contains("nvidia") && !gl.contains("nouveau") {
        Some("cuda".into())
    } else if [
        "mesa", "amd", "radeon", "intel", "x.org", "llvmpipe", "nouveau",
    ]
    .iter()
    .any(|k| gl.contains(k))
    {
        Some("vaapi".into())
    } else {
        None
    }
}

/// GL_VENDOR / GL_RENDERER of the current context (empty when unavailable).
fn gl_strings(gpa: ProcFn) -> (String, String) {
    type GetString = extern "system" fn(u32) -> *const u8;
    let p = gpa(c"glGetString");
    if p.is_null() {
        return Default::default();
    }
    // SAFETY: glGetString(GLenum) -> const GLubyte*, per the OpenGL specification.
    let f = unsafe { std::mem::transmute::<*const c_void, GetString>(p) };
    let get = |name: u32| {
        let p = f(name);
        if p.is_null() {
            return String::new();
        }
        // SAFETY: a non-null result is a NUL-terminated static string owned by the driver.
        unsafe { std::ffi::CStr::from_ptr(p.cast()) }
            .to_string_lossy()
            .into_owned()
    };
    (get(0x1F00), get(0x1F01))
}

// The mpv render context is created lazily, on the first frame after something wants
// to play (see [`when_ready`]). Creating it loads mpv's GL renderer and the hwdec
// interop (on NVIDIA: libcuda and the CUDA driver), which cost tens of MB while vivi
// sits idle with nothing open.

/// Set once anything asked for playback; from then on the context is (re)created as
/// soon as the window has a GL context.
static WANTED: AtomicBool = AtomicBool::new(false);
/// The context exists (cleared on rendering teardown).
static READY: AtomicBool = AtomicBool::new(false);
type Job = Box<dyn FnOnce() + Send>;
/// Commands waiting for the render context (files must load only after it exists,
/// otherwise `vo=libmpv` fails to initialize: "No render context set").
static WAITING: Mutex<Vec<Job>> = Mutex::new(Vec::new());

thread_local! {
    static WINDOW: std::cell::RefCell<Option<slint::Weak<crate::AppWindow>>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f` once the mpv render context exists: right away when it does, otherwise after
/// the next frame creates it. If no frame comes (window hidden to the tray) the jobs run
/// anyway after a short wait, so audio still plays.
pub fn when_ready(f: impl FnOnce() + Send + 'static) {
    if READY.load(Ordering::Acquire) {
        f();
        return;
    }
    WANTED.store(true, Ordering::Release);
    WAITING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Box::new(f));
    let _ = slint::invoke_from_event_loop(|| {
        WINDOW.with(|w| {
            if let Some(a) = w.borrow().as_ref().and_then(|w| w.upgrade()) {
                a.window().request_redraw();
            }
        });
        slint::Timer::single_shot(std::time::Duration::from_millis(500), run_waiting);
    });
}

fn run_waiting() {
    let jobs = std::mem::take(&mut *WAITING.lock().unwrap_or_else(|e| e.into_inner()));
    for j in jobs {
        j();
    }
}

/// Hook the mpv render context into the window's rendering. Files given on the command
/// line should be opened through [`when_ready`] (the player does this itself).
pub fn attach(
    app: &crate::AppWindow,
    mpv: Arc<Mpv>,
    interop: (String, String),
) -> Result<(), slint::SetRenderingNotifierError> {
    let weak = app.as_weak();
    WINDOW.with(|w| *w.borrow_mut() = Some(weak.clone()));
    let mut ctx: Option<RenderContext> = None;
    let mut gl: Option<GlReset> = None;
    app.window().set_rendering_notifier(move |state, api| {
        let GraphicsAPI::NativeOpenGL {
            get_proc_address: gpa,
        } = api
        else {
            if matches!(state, RenderingState::RenderingSetup) {
                eprintln!("vivi: non-OpenGL renderer, video output unavailable");
            }
            return;
        };
        let gpa: ProcFn = gpa;
        match state {
            RenderingState::RenderingSetup => {
                gl = GlReset::load(gpa);
                let (vendor, renderer) = gl_strings(gpa);
                if let Some(i) = pick_interop(&interop.0, &interop.1, &vendor, &renderer) {
                    if let Err(e) = mpv.set_property("gpu-hwdec-interop", i.as_str()) {
                        eprintln!("vivi: gpu-hwdec-interop={i}: {e:?}");
                    }
                }
                if WANTED.load(Ordering::Acquire) {
                    ctx = create(&mpv, gpa, &weak);
                }
            }
            RenderingState::BeforeRendering => {
                if ctx.is_none() && WANTED.load(Ordering::Acquire) {
                    ctx = create(&mpv, gpa, &weak);
                }
                if let (Some(rc), Some(a)) = (
                    &ctx,
                    weak.upgrade()
                        .filter(|a| a.global::<crate::Playback>().get_has_media()),
                ) {
                    let size = a.window().size();
                    if let Some(g) = &gl {
                        g.apply();
                    }
                    if let Err(e) =
                        rc.render::<usize>(0, size.width as i32, size.height as i32, true)
                    {
                        eprintln!("vivi: mpv render error: {e:?}");
                    }
                }
            }
            RenderingState::RenderingTeardown => {
                READY.store(false, Ordering::Release);
                ctx = None;
            }
            _ => {}
        }
    })
}

/// Create the render context (GL context current) and run the waiting jobs after this
/// rendering callback returns.
fn create(
    mpv: &Arc<Mpv>,
    gpa: ProcFn,
    weak: &slint::Weak<crate::AppWindow>,
) -> Option<RenderContext> {
    let ptr = &gpa as *const ProcFn as usize;
    // SAFETY: mpv handle outlives the render context (Arc kept in the notifier closure).
    let handle = unsafe { &mut *mpv.ctx.as_ptr() };
    match RenderContext::new(
        handle,
        vec![
            RenderParam::ApiType(RenderParamApiType::OpenGl),
            RenderParam::InitParams(OpenGLInitParams {
                get_proc_address,
                ctx: ptr,
            }),
        ],
    ) {
        Ok(mut rc) => {
            let w = weak.clone();
            rc.set_update_callback(move || {
                let w = w.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(a) = w.upgrade() {
                        a.window().request_redraw();
                    }
                });
            });
            READY.store(true, Ordering::Release);
            slint::Timer::single_shot(std::time::Duration::ZERO, run_waiting);
            Some(rc)
        }
        Err(e) => {
            eprintln!("vivi: mpv render context failed: {e:?}");
            // Do not retry every frame; queued files still load (audio plays).
            WANTED.store(false, Ordering::Release);
            slint::Timer::single_shot(std::time::Duration::ZERO, run_waiting);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pick_interop;

    #[test]
    fn interop_follows_config_hwdec_and_gl_vendor() {
        let p = |c, h, v, r| pick_interop(c, h, v, r);
        assert_eq!(
            p("vulkan", "auto-safe", "NVIDIA Corporation", ""),
            Some("vulkan".into())
        );
        if !cfg!(target_os = "linux") {
            assert_eq!(p("auto", "auto-safe", "AMD", "Radeon"), None);
            return;
        }
        assert_eq!(
            p(
                "auto",
                "auto-safe",
                "AMD",
                "AMD Radeon Graphics (radeonsi, renoir)"
            ),
            Some("vaapi".into())
        );
        assert_eq!(
            p("", "auto-safe", "Intel", "Mesa Intel(R) Xe Graphics"),
            Some("vaapi".into())
        );
        assert_eq!(
            p(
                "auto",
                "auto-safe",
                "Mesa",
                "llvmpipe (LLVM 19.1.7, 256 bits)"
            ),
            Some("vaapi".into())
        );
        assert_eq!(
            p(
                "auto",
                "auto",
                "NVIDIA Corporation",
                "NVIDIA GeForce RTX 3050"
            ),
            Some("cuda".into())
        );
        assert_eq!(
            p("auto", "auto-safe", "Mesa", "NV167 (nouveau)"),
            Some("vaapi".into())
        );
        assert_eq!(p("auto", "nvdec", "AMD", "Radeon"), Some("cuda".into()));
        assert_eq!(
            p("auto", "vaapi-copy", "NVIDIA Corporation", ""),
            Some("vaapi".into())
        );
        assert_eq!(p("auto", "vulkan", "AMD", "Radeon"), None);
        assert_eq!(p("auto", "auto-safe", "Some Vendor", "GPU"), None);
    }
}
