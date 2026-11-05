#!/usr/bin/env -S cargo +nightly -Zscript
---cargo
[package]
edition = "2024"
[dependencies]
serde_json = "1"
---
mod support;
use std::{env, path::PathBuf, process::Command};
use support::{Godot, Result, run};

fn main() {
    support::finish(execute());
}

fn execute() -> Result {
    let args: Vec<String> = env::args().skip(1).collect();
    if args
        .first()
        .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        println!(
            "cargo +nightly -Zscript scripts/godot-mcp.rs [--url URL | URL | --check]\nDefault URL: http://localhost:17860\n--check only checks configured executables and versions; it does not install or open an editor."
        );
        return Ok(());
    }
    let (check, url) = match args.as_slice() {
        [] => (false, "http://localhost:17860"),
        [flag] if flag == "--check" => (true, "http://localhost:17860"),
        [flag, url] if flag == "--url" || flag == "--server-url" => (false, url.as_str()),
        [url] if !url.starts_with('-') => (false, url.as_str()),
        _ => return Err("Use --help for godot-mcp arguments".into()),
    };
    let repo = support::repo_root(env!("CARGO_MANIFEST_PATH"))?;
    let godot = Godot::load()?;
    let project = repo.join("godot");
    let dotnet = support::setting("DOTNET_ROOT")?
        .map(PathBuf::from)
        .map(|p| {
            p.join(if cfg!(windows) {
                "dotnet.exe"
            } else {
                "dotnet"
            })
        })
        .unwrap_or_else(|| PathBuf::from("dotnet"));
    if check {
        println!(
            "Godot: {}\nProject: {}",
            godot.editor.display(),
            project.display()
        );
        run(support::godot_cli()?.arg("--version"))?;
        return run(support::hidden(Command::new(dotnet).arg("--version")));
    }
    if !project.join("addons/godot_mcp/plugin.cfg").is_file() {
        run(support::godot_cli()?
            .arg("install-plugin")
            .arg(&project)
            .args(["--version", "0.33.0"]))?;
    }
    let packages = godot
        .editor
        .parent()
        .ok_or("Godot executable has no parent directory")?
        .join("GodotSharp/Tools/nupkgs");
    let csproj = project.join("exa.Editor.csproj");
    let mut restore = Command::new(&dotnet);
    support::hidden(&mut restore)
        .current_dir(&repo)
        .arg("restore")
        .arg(&csproj)
        .arg("--source")
        .arg(packages)
        .args(["--source", "https://api.nuget.org/v3/index.json"]);
    godot.configure(&mut restore)?;
    run(&mut restore)?;
    let mut build = Command::new(&dotnet);
    support::hidden(&mut build)
        .current_dir(&repo)
        .arg("build")
        .arg(csproj)
        .args(["--no-restore", "--nologo", "--verbosity", "quiet"]);
    godot.configure(&mut build)?;
    run(&mut build)?;
    // Build/stage the native DLL with just build before opening this editor.
    let mut open = support::godot_cli()?;
    open.current_dir(&repo)
        .arg("open")
        .arg(project)
        .arg("--editor-path")
        .arg(&godot.editor)
        .args([
            "--headless",
            "--no-build",
            "--idle-timeout",
            "0",
            "--url",
            url,
        ]);
    godot.configure(&mut open)?;
    run(&mut open)
}
