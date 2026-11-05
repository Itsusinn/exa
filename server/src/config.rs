//! Project configuration names, with read compatibility for pre-exa launchers.
use std::{env, ffi::OsString};

fn lookup(suffix: &str, mut read: impl FnMut(&str) -> Option<OsString>) -> Option<OsString> {
    read(&format!("EXA_{suffix}")).or_else(|| read(&format!("CDDA_{suffix}")))
}

pub fn var_os(suffix: &str) -> Option<OsString> {
    lookup(suffix, |name| env::var_os(name))
}

pub fn var(suffix: &str) -> Result<String, env::VarError> {
    var_os(suffix)
        .ok_or(env::VarError::NotPresent)?
        .into_string()
        .map_err(env::VarError::NotUnicode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_configuration_takes_precedence_including_empty_values() {
        let values = [("EXA_SAVE_PATH", "new.bin"), ("CDDA_SAVE_PATH", "old.bin")];
        let read = |name: &str| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, v)| (*v).into())
        };
        assert_eq!(lookup("SAVE_PATH", read), Some("new.bin".into()));
        assert_eq!(
            lookup("SAVE_PATH", |key| (key == "CDDA_SAVE_PATH")
                .then(|| "old.bin".into())),
            Some("old.bin".into())
        );
        assert_eq!(
            lookup("SAVE_PATH", |_| Some(OsString::new())),
            Some(OsString::new())
        );
        assert_eq!(lookup("SAVE_PATH", |_| None), None);
    }
}
