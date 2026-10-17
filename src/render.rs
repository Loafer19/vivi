//! mpv render API (OpenGL) drawn as an underlay beneath the Slint scene.
//! Slint (femtovg) renders into the default framebuffer; in `BeforeRendering` we let mpv
//! draw the current video frame into FBO 0, then Slint paints the (transparent) UI on top.

use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};
use libmpv2::Mpv;
use slint::{ComponentHandle, GraphicsAPI, RenderingState};
use std::ffi::{c_void, CString};
use std::sync::Arc;

type ProcFn<'a> = &'a dyn Fn(&std::ffi::CStr) -> *const c_void;

fn get_proc_address(ctx: &usize, name: &str) -> *mut c_void {
    // SAFETY: ctx points at a `ProcFn` that is alive for the duration of
    // `RenderContext::new` (mpv resolves all GL symbols during creation).
    let f: &ProcFn = unsafe { &*(*ctx as *const ProcFn) };
    let name = CString::new(name).unwrap();
    f(&name) as *mut c_void
}

/// Minimal GL state reset before mpv draws. femtovg leaves textures, programs, buffers,
/// scissor/blend state and pixel-store settings bound after uploading images (library
/// thumbnails, seek previews), which made mpv's frame come out black.
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

/// `on_ready` runs once the mpv render context exists. Files must be loaded only after
/// that, otherwise `vo=libmpv` fails to initialize ("No render context set").
pub fn attach(
    app: &crate::AppWindow,
    mpv: Arc<Mpv>,
    on_ready: impl FnOnce() + 'static,
) -> Result<(), slint::SetRenderingNotifierError> {
    let weak = app.as_weak();
    let mut ctx: Option<RenderContext> = None;
    let mut gl: Option<GlReset> = None;
    let mut on_ready = Some(on_ready);
    app.window()
        .set_rendering_notifier(move |state, api| match state {
            RenderingState::RenderingSetup => {
                let GraphicsAPI::NativeOpenGL {
                    get_proc_address: gpa,
                } = api
                else {
                    eprintln!("vivi: non-OpenGL renderer, video output unavailable");
                    return;
                };
                let gpa: ProcFn = gpa;
                gl = GlReset::load(gpa);
                let ptr = &gpa as *const ProcFn as usize;
                // SAFETY: mpv handle outlives the render context (Arc kept in closure).
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
                        ctx = Some(rc);
                        if let Some(f) = on_ready.take() {
                            // Run after this callback returns (outside the GL setup).
                            slint::Timer::single_shot(std::time::Duration::ZERO, f);
                        }
                    }
                    Err(e) => eprintln!("vivi: mpv render context failed: {e:?}"),
                }
            }
            RenderingState::BeforeRendering => {
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
            RenderingState::AfterRendering => {
                let _ = &ctx;
            }
            RenderingState::RenderingTeardown => {
                ctx = None;
            }
            _ => {}
        })
}
