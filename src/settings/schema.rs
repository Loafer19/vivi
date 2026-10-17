//! Settings schema: one entry per stored setting id with how its value is read from and
//! written to [`Config`]. The row builder reads values through [`get`] and the
//! `setting-changed` handler writes them through [`set`], so an id is mapped in one place.
//! Categories move here one at a time; ids not listed still use the code in `settings.rs`.

use crate::config::Config;

pub(super) struct Field {
    pub id: &'static str,
    pub get: fn(&Config) -> String,
    pub set: fn(&mut Config, &str),
}

fn on(v: &str) -> bool {
    v == "true"
}

/// General category (log file path is handled with its file picker in `settings.rs`).
pub(super) const GENERAL: &[Field] = &[
    Field {
        id: "general.language",
        get: |c| c.general.language.clone(),
        set: |c, v| c.general.language = v.into(),
    },
    Field {
        id: "general.remote_control",
        get: |c| c.general.remote_control.to_string(),
        set: |c, v| c.general.remote_control = on(v),
    },
    Field {
        id: "general.save_log",
        get: |c| c.general.save_log.to_string(),
        set: |c, v| c.general.save_log = on(v),
    },
    Field {
        id: "general.log_level",
        get: |c| c.general.log_level.clone(),
        set: |c, v| c.general.log_level = v.into(),
    },
];

fn field(id: &str) -> Option<&'static Field> {
    GENERAL.iter().find(|f| f.id == id)
}

/// Current value of a schema setting (empty for ids not in the schema).
pub(super) fn get(c: &Config, id: &str) -> String {
    field(id).map(|f| (f.get)(c)).unwrap_or_default()
}

/// Whether `id` is stored through the schema.
pub(super) fn has(id: &str) -> bool {
    field(id).is_some()
}

/// Stores `v` for a schema setting; returns false for ids not in the schema.
pub(super) fn set(c: &mut Config, id: &str, v: &str) -> bool {
    field(id).map(|f| (f.set)(c, v)).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip() {
        let mut c = Config::default();
        for (id, v) in [
            ("general.language", "English"),
            ("general.remote_control", "true"),
            ("general.save_log", "true"),
            ("general.log_level", "Everything"),
        ] {
            assert!(set(&mut c, id, v), "{id}");
            assert_eq!(get(&c, id), v, "{id}");
        }
        assert!(c.general.remote_control && c.general.save_log);
        assert!(!set(&mut c, "general.nope", "x"));
        assert!(GENERAL.iter().all(|f| has(f.id)));
    }
}
