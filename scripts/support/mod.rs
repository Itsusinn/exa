//! Shared implementation for the Cargo script entry points (not a standalone script).
#![allow(dead_code)] // Each entry point uses a different subset of these helpers.
use std::{
    env,
    error::Error,
    ffi::{OsStr, OsString},
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

#[derive(Debug)]
struct CommandFailure(String, ExitStatus);
impl std::fmt::Display for CommandFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} exited with {}", self.0, self.1)
    }
}
impl Error for CommandFailure {}

pub fn finish(result: Result) {
    if let Err(error) = result {
        eprintln!("{error}");
        let code = error
            .downcast_ref::<CommandFailure>()
            .and_then(|e| e.1.code())
            .filter(|code| *code != 0)
            .unwrap_or(1);
        std::process::exit(code);
    }
}

pub fn hidden(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // No extra console for helper processes.
    }
    command
}

pub fn run(command: &mut Command) -> Result {
    let program = command.get_program().to_string_lossy().into_owned();
    let status = command
        .status()
        .map_err(|e| format!("Cannot start {program}: {e}"))?;
    if !status.success() {
        return Err(Box::new(CommandFailure(program, status)));
    }
    Ok(())
}

pub fn capture(command: &mut Command) -> Result<Vec<u8>> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("Cannot start {program}: {e}"))?;
    if !output.status.success() {
        return Err(Box::new(CommandFailure(program, output.status)));
    }
    Ok(output.stdout)
}

/// Cargo exposes the absolute source manifest path for single-file scripts.
pub fn repo_root(script: &str) -> Result<PathBuf> {
    let source = PathBuf::from(script);
    if !source.is_absolute() || !source.is_file() {
        return Err(format!("Invalid script manifest path: {script}").into());
    }
    let root = source
        .parent()
        .and_then(Path::parent)
        .ok_or("Script has no repository parent")?;
    if !root.join("Cargo.toml").is_file() || !root.join("godot/project.godot").is_file() {
        return Err("Script must belong to the game repository".into());
    }
    Ok(root.to_path_buf())
}

pub fn cargo(repo: &Path) -> Command {
    let mut command = Command::new("cargo");
    // Cargo script runs on nightly; project Cargo must honor rust-toolchain.toml.
    command.current_dir(repo).env_remove("RUSTUP_TOOLCHAIN");
    hidden(&mut command);
    command
}

pub fn target_directory(repo: &Path) -> Result<PathBuf> {
    let output = capture(cargo(repo).args(["metadata", "--no-deps", "--format-version", "1"]))?;
    let metadata: serde_json::Value = serde_json::from_slice(&output)?;
    Ok(metadata["target_directory"]
        .as_str()
        .ok_or("Cargo metadata omitted target_directory")?
        .into())
}

/// Read Windows user environment without spawning PowerShell or depending on its encoding.
#[cfg(windows)]
fn user_environment(name: &str) -> Result<Option<OsString>> {
    use std::{os::windows::ffi::OsStringExt, ptr};
    #[link(name = "Advapi32")]
    unsafe extern "system" {
        fn RegGetValueW(
            key: isize,
            subkey: *const u16,
            name: *const u16,
            flags: u32,
            kind: *mut u32,
            data: *mut std::ffi::c_void,
            size: *mut u32,
        ) -> i32;
    }
    let subkey: Vec<u16> = "Environment\0".encode_utf16().collect();
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let key = 0x8000_0001u32 as i32 as isize; // HKEY_CURRENT_USER, sign-extended.
    let mut bytes = 0;
    let flags = 0x2 | 0x4; // REG_SZ and REG_EXPAND_SZ (expanded by Windows).
    let status = unsafe {
        RegGetValueW(
            key,
            subkey.as_ptr(),
            name.as_ptr(),
            flags,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut bytes,
        )
    };
    if status == 2 {
        return Ok(None);
    }
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status).into());
    }
    let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
    let status = unsafe {
        RegGetValueW(
            key,
            subkey.as_ptr(),
            name.as_ptr(),
            flags,
            ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status).into());
    }
    buffer.truncate(bytes as usize / 2);
    while buffer.last() == Some(&0) {
        buffer.pop();
    }
    Ok((!buffer.is_empty()).then(|| OsString::from_wide(&buffer)))
}

#[cfg(not(windows))]
fn user_environment(_: &str) -> Result<Option<OsString>> {
    Ok(None)
}

pub fn setting(name: &str) -> Result<Option<OsString>> {
    match env::var_os(name).filter(|value| !value.is_empty()) {
        Some(value) => Ok(Some(value)),
        None => user_environment(name),
    }
}

pub struct Godot {
    pub editor: PathBuf,
    pub console: PathBuf,
    sdk: Option<OsString>,
}
impl Godot {
    pub fn load() -> Result<Self> {
        let editor = PathBuf::from(
            setting("GODOT_BIN")?.ok_or("Set GODOT_BIN to the Godot 4.7.2 executable")?,
        );
        if !editor.is_file() {
            return Err(format!("Godot executable not found: {}", editor.display()).into());
        }
        let sibling = editor.with_file_name(format!(
            "{}_console.exe",
            editor
                .file_stem()
                .ok_or("Godot executable has no filename")?
                .to_string_lossy()
        ));
        let console = if sibling.is_file() {
            sibling
        } else {
            editor.clone()
        };
        Ok(Self {
            editor,
            console,
            sdk: setting("DOTNET_ROOT")?,
        })
    }

    pub fn configure(&self, command: &mut Command) -> Result {
        if let Some(sdk) = &self.sdk {
            let paths = std::iter::once(PathBuf::from(sdk))
                .chain(env::split_paths(&env::var_os("PATH").unwrap_or_default()))
                .collect::<Vec<_>>();
            command
                .env("DOTNET_ROOT", sdk)
                .env("PATH", env::join_paths(paths)?);
        }
        Ok(())
    }

    pub fn command(&self, project: &Path) -> Result<Command> {
        let mut command = Command::new(&self.console);
        hidden(&mut command).args([OsStr::new("--path"), project.as_os_str()]);
        self.configure(&mut command)?;
        Ok(command)
    }
}

pub fn same_file_contents(a: &Path, b: &Path) -> io::Result<bool> {
    if !b.exists() || fs::metadata(a)?.len() != fs::metadata(b)?.len() {
        return Ok(false);
    }
    let mut a = fs::File::open(a)?;
    let mut b = fs::File::open(b)?;
    let (mut ab, mut bb) = ([0u8; 65536], [0u8; 65536]);
    loop {
        let n = a.read(&mut ab)?;
        if n == 0 {
            return Ok(true);
        }
        b.read_exact(&mut bb[..n])?;
        if ab[..n] != bb[..n] {
            return Ok(false);
        }
    }
}

pub fn build_extension(repo: &Path, target: &Path, release: bool) -> Result {
    let profile = if release { "release" } else { "debug" };
    let mut command = cargo(repo);
    command.args(["build", "-p", "game-godot", "--jobs", "2"]);
    if release {
        command.arg("--release");
    }
    run(&mut command)?;
    let dest = repo.join("godot/bin").join(profile).join("game_godot.dll");
    let source = target.join(profile).join("game_godot.dll");
    fs::create_dir_all(dest.parent().unwrap())?;
    if !same_file_contents(&source, &dest)? {
        fs::copy(&source, &dest).map_err(|e| {
            format!(
                "Close the Godot editor and game before replacing {}: {e}",
                dest.display()
            )
        })?;
    }
    Ok(())
}

pub fn unique_directory(parent: &Path, prefix: &str) -> Result<PathBuf> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    fs::create_dir_all(parent)?;
    loop {
        let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let next = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!("{prefix}-{}-{time}-{next}", std::process::id()));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
}

pub fn copy_tree(source: &Path, dest: &Path) -> Result {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(format!("Refusing staging symlink: {}", source.display()).into());
    }
    if metadata.is_dir() {
        fs::create_dir_all(dest)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &dest.join(entry.file_name()))?;
        }
    } else {
        fs::copy(source, dest)?;
    }
    Ok(())
}

pub fn runtime_project(text: &str) -> String {
    let mut skip = false;
    let mut output = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            skip = matches!(trimmed, "[dotnet]" | "[editor_plugins]");
        }
        if !skip {
            output.push_str(line);
            output.push('\n');
        }
    }
    output
}

/// Closing stdin requests the authority's normal final-save shutdown, also on early error.
pub struct Authority(pub Child);
impl Authority {
    pub fn stop(&mut self) -> Result {
        drop(self.0.stdin.take());
        let status = self.0.wait()?;
        if !status.success() {
            return Err(Box::new(CommandFailure(
                "Server (final save)".into(),
                status,
            )));
        }
        Ok(())
    }
}
impl Drop for Authority {
    fn drop(&mut self) {
        drop(self.0.stdin.take());
        let _ = self.0.wait();
    }
}

/// npm's Windows shim needs a shell. Resolve its package entry point and invoke Node directly.
pub fn godot_cli() -> Result<Command> {
    for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        if cfg!(windows) {
            let package = directory.join("node_modules/godot-cli/package.json");
            if !package.is_file() {
                continue;
            }
            let metadata: serde_json::Value = serde_json::from_slice(&fs::read(&package)?)?;
            let entry = metadata["bin"]
                .as_str()
                .or_else(|| metadata["bin"]["godot-cli"].as_str())
                .ok_or("godot-cli package has no CLI entry point")?;
            let local_node = directory.join("node.exe");
            let mut command = Command::new(if local_node.is_file() {
                local_node
            } else {
                PathBuf::from("node")
            });
            hidden(&mut command).arg(package.parent().unwrap().join(entry));
            return Ok(command);
        } else {
            let cli = directory.join("godot-cli");
            if cli.is_file() {
                return Ok(Command::new(cli));
            }
        }
    }
    Err("Install godot-cli: npm install -g godot-cli@0.33.0".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_strips_editor_sections_without_removing_runtime_settings() {
        let source = "config_version=5\r\n[application]\r\nconfig/name=\"示例\"\r\n[dotnet]\r\nproject/assembly_name=\"Editor\"\r\n[rendering]\r\nrenderer=\"forward_plus\"\r\n[editor_plugins]\r\nenabled=[\"secret editor endpoint\"]\r\n";
        let clean = runtime_project(source);
        assert!(clean.contains("config/name=\"示例\""));
        assert!(clean.contains("[rendering]\nrenderer=\"forward_plus\""));
        assert!(!clean.contains("Editor") && !clean.contains("endpoint"));
        assert_eq!(runtime_project(&clean), clean);
    }

    #[test]
    fn staged_copy_preserves_unicode_paths_and_dll_comparison_detects_same_length_changes() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target/script-tests");
        let fixture = unique_directory(&root, "copy").unwrap();
        let source = fixture.join("源 目录");
        let dest = fixture.join("输出 目录");
        fs::create_dir_all(source.join("sub")).unwrap();
        fs::write(source.join("sub/a.dll"), b"test").unwrap();
        copy_tree(&source, &dest).unwrap();
        let a = source.join("sub/a.dll");
        let b = dest.join("sub/a.dll");
        assert!(same_file_contents(&a, &b).unwrap());
        fs::write(&b, b"next").unwrap();
        assert!(!same_file_contents(&a, &b).unwrap());
        assert_eq!(fs::read(&a).unwrap(), b"test");
    }
}
