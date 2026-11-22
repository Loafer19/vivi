//! Start-up diagnostics that must reach the user even without a terminal.
//!
//! * On Windows `vivi.exe` is a GUI-subsystem program, so it has no console of its own:
//!   [`attach_console`] attaches to the console it was started from (cmd, PowerShell), so
//!   `--version`, `--help` and error messages show there. Started from Explorer or the
//!   Start menu there is no parent console and nothing changes.
//! * [`note`] appends a line to `vivi-startup.log` in the cache folder (the renderer that
//!   was picked, why a fallback happened).
//! * [`report`] is for errors that end the program: logged, printed, and on Windows shown
//!   in a message box (otherwise vivi would just vanish when started from Explorer).

use std::io::Write as _;
use std::path::PathBuf;

/// `<cache>/vivi-startup.log` (next to mpv's `vivi.log`).
pub(crate) fn log_path() -> PathBuf {
    crate::store::cache_dir().join("vivi-startup.log")
}

/// Append `msg` to the start-up log and print it to stderr.
pub(crate) fn note(msg: &str) {
    eprintln!("vivi: {msg}");
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Keep the log small: start over once it passes 64 KiB.
    let append = std::fs::metadata(&path).is_ok_and(|m| m.len() < 64 * 1024);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(&path);
    if let Ok(mut f) = file {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(
            f,
            "[{secs}] vivi {} pid {}: {msg}",
            env!("CARGO_PKG_VERSION"),
            std::process::id()
        );
    }
}

/// An error that ends the program: logged, printed and (Windows) shown in a message box.
pub(crate) fn report(msg: &str) {
    note(&format!("fatal: {msg}"));
    #[cfg(windows)]
    win::message_box(
        "Vivi could not start",
        &format!(
            "{msg}\n\nDetails were written to:\n{}",
            log_path().display()
        ),
    );
}

/// Panics abort in release builds; log the message (and show it on Windows) first.
pub(crate) fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        report(&format!("internal error: {info}"));
    }));
}

/// Windows: attach to the parent process's console when there is one and the standard
/// handles were not redirected (a pipe or file given by the caller is kept as is).
pub(crate) fn attach_console() {
    #[cfg(windows)]
    win::attach_console();
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const MB_OK: u32 = 0x0;
    const MB_ICONERROR: u32 = 0x10;
    const MB_SETFOREGROUND: u32 = 0x1_0000;

    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
        fn GetStdHandle(std_handle: u32) -> *mut c_void;
    }
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: *mut c_void, text: *const u16, caption: *const u16, ty: u32) -> i32;
    }

    fn missing(h: *mut c_void) -> bool {
        h.is_null() || h as isize == -1
    }

    pub(super) fn attach_console() {
        // SAFETY: plain Win32 calls without pointers; failure (no parent console) is fine.
        unsafe {
            if missing(GetStdHandle(STD_OUTPUT_HANDLE)) || missing(GetStdHandle(STD_ERROR_HANDLE)) {
                AttachConsole(ATTACH_PARENT_PROCESS);
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub(super) fn message_box(caption: &str, text: &str) {
        let (c, t) = (wide(caption), wide(text));
        // SAFETY: both strings are NUL-terminated UTF-16 and outlive the call.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                t.as_ptr(),
                c.as_ptr(),
                MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
            );
        }
    }
}
