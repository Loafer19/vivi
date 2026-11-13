//! Customize dialog (toolbar layout and interface profile).

use super::*;

pub(crate) fn customize(
    app: &AppWindow,
    player: &Rc<Player>,
    cfg: &AppState,
    save: impl Fn() + Clone + 'static,
    log: bool,
) {
    let backup: Rc<RefCell<Option<config::Interface>>> = Rc::new(RefCell::new(None));
    let edit = {
        let w = app.as_weak();
        let cfg = cfg.clone();
        move |f: &dyn Fn(&mut Vec<String>)| {
            let Some(a) = w.upgrade() else { return };
            // Plugin buttons come from the plugin host, which reads the config: ask
            // before borrowing it mutably, and apply after the borrow ends (0.16.3
            // aborted with "RefCell already mutably borrowed" here).
            let buttons = plugin_buttons();
            let mut t = visible_toolbar(&cfg.config().interface.toolbar, &buttons);
            f(&mut t);
            cfg.send(AppMsg::SetToolbar(t));
            apply_toolbar(&a, &cfg.config());
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
                    *backup.borrow_mut() = Some(cfg.config().interface.clone());
                    a.global::<crate::Overlays>().set_customize_open(true);
                }
                cfg.send(AppMsg::SelectProfile(p.to_string()));
                apply_toolbar(&a, &cfg.config());
            });
    }
    {
        let edit = edit.clone();
        app.global::<crate::Customize>().on_move(move |i, d| {
            edit(&|t| {
                let (i, j) = (i as usize, i as usize as i64 + d as i64);
                // Move item i to position j (a neighbour swap for d = ±1, a drag otherwise).
                if j >= 0 && (j as usize) < t.len() && i < t.len() {
                    let it = t.remove(i);
                    t.insert(j as usize, it);
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
            cfg.send(AppMsg::SetBarStyle(st.to_string()));
            apply_toolbar(&a, &cfg.config());
        });
    }
    {
        let w = app.as_weak();
        let cfg = cfg.clone();
        app.global::<crate::Customize>().on_pick_volume(move |st| {
            let Some(a) = w.upgrade() else { return };
            cfg.send(AppMsg::SetVolumeStyle(
                config::volume_style_name(&st).into(),
            ));
            apply_toolbar(&a, &cfg.config());
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
                    eprintln!("toolbar saved: {:?}", cfg.config().interface.toolbar);
                }
            } else if let Some(old) = old {
                cfg.send(AppMsg::RestoreInterface(Box::new(old)));
            }
            apply_toolbar(&a, &cfg.config());
        });
        let w = app.as_weak();
        app.global::<crate::Playback>().on_bar_action(move |id| {
            if let Some(a) = w.upgrade() {
                bar_action(&a, &p, &id);
            }
        });
    }
}
