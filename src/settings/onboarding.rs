//! First-run onboarding dialog.

use super::*;

pub(crate) fn onboarding(app: &AppWindow, cfg: &Rc<RefCell<Config>>, save: impl Fn() + 'static) {
    use crate::keymap::Action as A;
    let keys = crate::keymap::rows_of(&[
        A::PlayPause,
        A::SeekForward,
        A::VolumeUp,
        A::Fullscreen,
        A::OpenFile,
        A::Shortcuts,
    ]);
    app.global::<crate::Settings>()
        .set_onboarding_actions(strings(keys.iter().map(|k| k.0)));
    app.global::<crate::Settings>()
        .set_onboarding_keys(strings(keys.iter().map(|k| k.1)));
    app.global::<crate::Overlays>()
        .set_onboarding_open(!cfg.borrow().interface.onboarded);
    // The `?` overlay lists the whole keymap.
    let all: Vec<_> = crate::keymap::rows().collect();
    app.global::<crate::Settings>()
        .set_shortcut_actions(strings(all.iter().map(|k| k.0)));
    app.global::<crate::Settings>()
        .set_shortcut_keys(strings(all.iter().map(|k| k.1)));
    let w = app.as_weak();
    let cfg = cfg.clone();
    app.global::<crate::Settings>().on_onboarding_done(move || {
        if let Some(a) = w.upgrade() {
            a.global::<crate::Overlays>().set_onboarding_open(false);
        }
        cfg.borrow_mut().interface.onboarded = true;
        save();
    });
}
