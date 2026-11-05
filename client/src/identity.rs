//! Local UUID, independent of nickname. Called only on the network worker.
use anyhow::{Context, Result};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub fn default_path() -> PathBuf {
    if let Some(path) = exa_server::config::var_os("IDENTITY_PATH") {
        return path.into();
    }
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("save"));
    default_path_in(&base)
}

fn default_path_in(base: &Path) -> PathBuf {
    let current = base.join("exa/client.uuid");
    let legacy = base.join("Dungeon3D/client.uuid");
    // An unreadable or malformed identity must fail when opened, never create a new player.
    let present = |path: &Path| match std::fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    };
    if !present(&current) && present(&legacy) {
        legacy
    } else {
        current
    }
}

pub fn load_or_create(path: &Path) -> Result<[u8; 16]> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    // Serialize simultaneous first launches; a partial/corrupt identity is never
    // silently replaced with a different character.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("uuid.lock"))?;
    lock.lock().context("lock client identity")?;
    match std::fs::File::open(path) {
        Ok(file) => {
            anyhow::ensure!(
                file.metadata()?.len() <= 64,
                "invalid client identity length"
            );
            let mut text = String::new();
            file.take(65).read_to_string(&mut text)?;
            let id = uuid::Uuid::parse_str(text.trim())
                .context("invalid client UUID; original identity preserved")?;
            anyhow::ensure!(!id.is_nil(), "nil client UUID");
            Ok(*id.as_bytes())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let id = uuid::Uuid::new_v4();
            let temporary = path.with_extension("uuid.tmp");
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            writeln!(file, "{id}")?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(temporary, path)?;
            Ok(*id.as_bytes())
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_preserves_legacy_identity_and_prefers_an_existing_exa_identity() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/identity-tests")
            .join(uuid::Uuid::new_v4().to_string());
        let current = root.join("exa/client.uuid");
        let legacy = root.join("Dungeon3D/client.uuid");
        assert_eq!(default_path_in(&root), current);
        let old_id = load_or_create(&legacy).unwrap();
        assert_eq!(default_path_in(&root), legacy);
        assert_eq!(load_or_create(&default_path_in(&root)).unwrap(), old_id);
        std::fs::write(&legacy, "broken identity").unwrap();
        assert!(load_or_create(&default_path_in(&root)).is_err());
        assert!(!current.exists());
        let new_id = load_or_create(&current).unwrap();
        assert_eq!(default_path_in(&root), current);
        assert_eq!(load_or_create(&default_path_in(&root)).unwrap(), new_id);
        assert_eq!(std::fs::read_to_string(legacy).unwrap(), "broken identity");
    }
    #[test]
    fn identity_survives_reopen_and_corruption_is_not_replaced() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/identity-tests")
            .join(uuid::Uuid::new_v4().to_string());
        let path = root.join("client.uuid");
        let id = load_or_create(&path).unwrap();
        assert_ne!(id, [0; 16]);
        assert_eq!(load_or_create(&path).unwrap(), id);
        std::fs::write(&path, "broken identity").unwrap();
        assert!(load_or_create(&path).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "broken identity");
    }
}
