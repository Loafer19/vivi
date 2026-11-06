//! The crate's error type, replacing `Result<_, String>` step by step (no extra
//! dependencies). Most errors end up as a message for the user or a control-socket
//! client, so every variant displays as plain text.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// A message for the user (bad arguments, nothing playing, plugin off, …).
    Msg(String),
    Io(std::io::Error),
    Db(rusqlite::Error),
    Json(serde_json::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Msg(m) => f.write_str(m),
            Self::Io(e) => e.fmt(f),
            Self::Db(e) => write!(f, "library database: {e}"),
            Self::Json(e) => write!(f, "JSON: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Msg(_) => None,
            Self::Io(e) => Some(e),
            Self::Db(e) => Some(e),
            Self::Json(e) => Some(e),
        }
    }
}

impl From<String> for Error {
    fn from(m: String) -> Self {
        Self::Msg(m)
    }
}

impl From<&str> for Error {
    fn from(m: &str) -> Self {
        Self::Msg(m.to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// For code that still passes errors on as `String`.
impl From<Error> for String {
    fn from(e: Error) -> Self {
        e.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_as_plain_text() {
        let e: Error = "nothing is playing".into();
        assert_eq!(e.to_string(), "nothing is playing");
        let e: Error = std::io::Error::new(std::io::ErrorKind::NotFound, "gone").into();
        assert_eq!(String::from(e), "gone");
        fn q() -> Result<()> {
            Err(format!("no track {}", 3))?
        }
        assert_eq!(q().unwrap_err().to_string(), "no track 3");
    }
}
