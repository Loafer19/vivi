//! Small helpers shared across the app and plugins: clock formatting, string models
//! for Slint, and [`Task`], a background job polled from the UI thread.

use slint::{ModelRc, SharedString, VecModel};
use std::sync::mpsc::{channel, Receiver};

/// Whole seconds → "m:ss", or "h:mm:ss" from one hour up.
pub fn clock(s: i64) -> String {
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Seconds → "HH:MM:SS", always zero-padded (side-panel chapter list).
pub fn hms(s: f64) -> String {
    let s = s.max(0.0) as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
}

/// A Slint string list model from anything that converts to `SharedString`.
pub fn strings(v: impl IntoIterator<Item = impl Into<SharedString>>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        v.into_iter().map(Into::into).collect::<Vec<_>>(),
    ))
}

/// A one-shot background job: [`Task::spawn`] runs `f` on a thread, [`Task::poll`]
/// (called from a UI timer) yields its result once. Dropping or [`Task::cancel`]ling it
/// discards a result that has not arrived yet.
pub struct Task<T> {
    rx: Option<Receiver<T>>,
}

impl<T> Default for Task<T> {
    fn default() -> Self {
        Task { rx: None }
    }
}

impl<T: Send + 'static> Task<T> {
    pub fn spawn(f: impl FnOnce() -> T + Send + 'static) -> Self {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        Task { rx: Some(rx) }
    }
}

impl<T> Task<T> {
    /// The result, once, if it has arrived.
    pub fn poll(&mut self) -> Option<T> {
        let v = self.rx.as_ref()?.try_recv().ok()?;
        self.rx = None;
        Some(v)
    }

    /// Whether a result is still awaited.
    pub fn is_running(&self) -> bool {
        self.rx.is_some()
    }

    /// Stop waiting; a late result is dropped.
    pub fn cancel(&mut self) {
        self.rx = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_formats() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(65), "1:05");
        assert_eq!(clock(3725), "1:02:05");
        assert_eq!(hms(3725.9), "01:02:05");
    }

    #[test]
    fn task_yields_once() {
        let mut t = Task::spawn(|| 7);
        assert!(t.is_running());
        let mut got = None;
        for _ in 0..200 {
            if let Some(v) = t.poll() {
                got = Some(v);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(got, Some(7));
        assert!(!t.is_running());
        assert_eq!(t.poll(), None);
    }
}
