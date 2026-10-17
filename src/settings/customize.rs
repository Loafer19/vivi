//! Customize dialog (toolbar layout and interface profile).

use super::*;

pub(crate) fn customize(
    app: &AppWindow,
    player: &Rc<Player>,
    cfg: &Rc<RefCell<Config>>,
    save: impl Fn() + Clone + 'static,
    log: bool,
) {
    let backup: Rc<RefCell<Option<config::Interface>>> = Rc::new(RefCell::new(None));
    let edit = {
        let w = app.as_weak();
        let cfg = cfg.clone();
        move |f: &dyn Fn(&mut Vec<String>)| {
            let Some(a) = w.upgrade() else { return };
            let mut c = cfg.borrow_mut();
            let mut t = config::sanitize_toolbar(&c.interface.toolbar);
            f(&mut t);
            c.interface.toolbar = t;
            c.interface.profile = profile_of(&c.interface);
            apply_toolbar(&a, &c);
        }
    };
    {
        let w = app.as_weak();
        let cfg = cfg.clone();
        let backup = backup.clone();
        app.global::<crate::Customize>()
            .on_select_profile(move |p| {
                let Some(a) = w.upgrade() else { return };
                if !a.global::<crate::Overlays>().get_customize_open() {
                    *backup.borrow_mut() = Some(cfg.borrow().interface.clone());
                    a.global::<crate::Overlays>().set_customize_open(true);
                }
                if let Some(t) = config::profile_toolbar(&p) {
                    let mut c = cfg.borrow_mut();
                    c.interface.toolbar = t;
                    c.interface.bar_style = config::profile_style(&p).into();
                    c.interface.profile = p.to_string();
                }
                apply_toolbar(&a, &cfg.borrow());
            });
    }
    {
        let edit = edit.clone();
        app.global::<crate::Customize>().on_move(move |i, d| {
            edit(&|t| {
                let (i, j) = (i as usize, i as usize as i64 + d as i64);
                if j >= 0 && (j as usize) < t.len() && i < t.len() {
                    t.swap(i, j as usize);
                }
            })
        });
    }
    {
        let edit = edit.clone();
        app.global::<crate::Customize>().on_remove(move |i| {
            edit(&|t| {
                if (i as usize) < t.len() {
                    t.remove(i as usize);
                }
            })
        });
    }
    {
        let w = app.as_weak();
        let cfg = cfg.clone();
        app.global::<crate::Customize>().on_style(move |st| {
            let Some(a) = w.upgrade() else { return };
            let mut c = cfg.borrow_mut();
            c.interface.bar_style = st.to_string();
            c.interface.profile = profile_of(&c.interface);
            apply_toolbar(&a, &c);
        });
    }
    app.global::<crate::Customize>()
        .on_add(move |id| edit(&|t| t.push(id.to_string())));
    {
        let w = app.as_weak();
        let cfg = cfg.clone();
        let p = player.clone();
        app.global::<crate::Customize>().on_done(move |keep| {
            let Some(a) = w.upgrade() else { return };
            a.global::<crate::Overlays>().set_customize_open(false);
            let old = backup.borrow_mut().take();
            if keep {
                save();
                if log {
                    eprintln!("toolbar saved: {:?}", cfg.borrow().interface.toolbar);
                }
            } else if let Some(old) = old {
                cfg.borrow_mut().interface = old;
            }
            apply_toolbar(&a, &cfg.borrow());
        });
        let w = app.as_weak();
        app.global::<crate::Playback>().on_bar_action(move |id| {
            if let Some(a) = w.upgrade() {
                bar_action(&a, &p, &id);
            }
        });
    }
}
