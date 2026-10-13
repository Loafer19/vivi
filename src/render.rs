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

pub fn attach(app: &crate::AppWindow, mpv: Arc<Mpv>) -> Result<(), slint::SetRenderingNotifierError> {
    let weak = app.as_weak();
    let mut ctx: Option<RenderContext> = None;
    app.window().set_rendering_notifier(move |state, api| match state {
        RenderingState::RenderingSetup => {
            let GraphicsAPI::NativeOpenGL { get_proc_address: gpa } = api else {
                eprintln!("vivi: non-OpenGL renderer, video output unavailable");
                return;
            };
            let gpa: ProcFn = gpa;
            let ptr = &gpa as *const ProcFn as usize;
            // SAFETY: mpv handle outlives the render context (Arc kept in closure).
            let handle = unsafe { &mut *mpv.ctx.as_ptr() };
            match RenderContext::new(
                handle,
                vec![
                    RenderParam::ApiType(RenderParamApiType::OpenGl),
                    RenderParam::InitParams(OpenGLInitParams { get_proc_address, ctx: ptr }),
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
                }
                Err(e) => eprintln!("vivi: mpv render context failed: {e:?}"),
            }
        }
        RenderingState::BeforeRendering => {
            if let (Some(rc), Some(a)) = (&ctx, weak.upgrade()) {
                let size = a.window().size();
                let _ = rc.render::<usize>(0, size.width as i32, size.height as i32, true);
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
