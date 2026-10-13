slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let app = AppWindow::new()?;
    let weak = app.as_weak();
    app.on_toggle_play(move || {
        if let Some(app) = weak.upgrade() {
            app.set_playing(!app.get_playing());
            // TODO: wire to libmpv2::Mpv (player module)
        }
    });
    app.run()
}
