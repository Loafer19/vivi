//! Control-socket commands: ui.

use super::*;
use crate::error::Error;
use crate::player::Player;
use crate::AppWindow;

pub(super) fn drawer(a: &AppWindow, _p: &Player, args: &Value) -> Result<Value, Error> {
    // Open / close / toggle the plugin side drawer (screenshots, scripts); it
    // shows while media plays.
    let open_it = drawer_open_arg(&args["open"])?;
    let g = a.global::<crate::Drawer>();
    let id = match args["plugin"].as_str() {
        Some(id) => id.to_string(),
        None if a.global::<crate::Overlays>().get_drawer_open() => g.get_tab().into(),
        None => String::new(),
    };
    if !id.is_empty()
        && !crate::plugins::contributions(crate::plugins::contrib::DRAWER_PANEL)
            .iter()
            .any(|e| e.id == id)
    {
        return Err(format!("plugin {id:?} has no drawer panel (or is turned off)").into());
    }
    let open = crate::drawer::request(a, &id, open_it);
    let tabs: Vec<String> = g.get_tabs().iter().map(|t| t.id.to_string()).collect();
    Ok(
        json!({ "open": open, "tab": g.get_tab().as_str(), "tabs": tabs,
        "shown": open && a.global::<crate::Playback>().get_has_media() }),
    )
}

pub(super) fn page(a: &AppWindow, _p: &Player, args: &Value) -> Result<Value, Error> {
    // Open a Streams hub place (#86): {"page": "streams:radio"} etc. (scripts,
    // screenshots; see src/streams/route.rs).
    let page = args["page"].as_str().unwrap_or("streams");
    if crate::streams::route::parse(page).is_none() {
        return Err(format!("unknown page {page:?}").into());
    }
    crate::streams::open_page(a, page);
    Ok(json!({ "page": page }))
}
