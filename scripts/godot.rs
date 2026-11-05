#!/usr/bin/env -S cargo +nightly -Zscript
---cargo
[package]
edition = "2024"
[dependencies]
serde_json = "1"
---
mod support;
use std::{
    env,
    ffi::OsString,
    fs,
    io::Write,
    net::UdpSocket,
    path::Path,
    process::Stdio,
    thread,
    time::{Duration, Instant},
};
use support::{Authority, Godot, Result, cargo, run};

const ACTIONS: &[&str] = &[
    "build",
    "run",
    "demo",
    "editor",
    "smoke",
    "export-debug",
    "export-release",
    "multiplayer",
    "containers",
    "editing",
    "industry",
    "industry-graphics",
    "industry-flow",
    "industry-flow-graphics",
    "industry-thermal",
    "industry-thermal-graphics",
    "industry-route",
    "industry-route-graphics",
    "industry-design",
    "industry-design-graphics",
    "distillation",
    "distillation-graphics",
    "column",
    "column-graphics",
    "column-loop",
    "column-loop-graphics",
    "streaming",
    "commands",
    "streaming-perf",
    "ecology",
    "structures",
    "natural-spawn",
];

fn main() {
    support::finish(execute());
}

fn execute() -> Result {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    if args
        .first()
        .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        println!(
            "cargo +nightly -Zscript scripts/godot.rs [ACTION] [GAME_ARGS...]\nActions: {}\nDefault: run. Game arguments are forwarded after Godot's -- separator.\nBuilds use the repository's stable toolchain; no extra console windows.",
            ACTIONS.join(", ")
        );
        return Ok(());
    }
    let action = args
        .first()
        .map(|a| a.to_str().ok_or("Action must be UTF-8"))
        .transpose()?
        .unwrap_or("run");
    if !ACTIONS.contains(&action) {
        return Err(format!("Unknown action {action}; use --help").into());
    }
    if args.len() > 1 && !matches!(action, "run" | "demo") {
        return Err("Only run/demo accept game arguments".into());
    }
    let repo = support::repo_root(env!("CARGO_MANIFEST_PATH"))?;
    env::set_current_dir(&repo)?;
    let target = support::target_directory(&repo)?;
    let godot = Godot::load()?;
    support::build_extension(&repo, &target, false)?;
    let project = repo.join("godot");
    match action {
        "build" => Ok(()),
        "run" | "demo" => {
            let mut command = godot.command(&project)?;
            command.arg("--");
            if action == "demo" {
                command.arg("--demo");
            }
            run(command.args(args.iter().skip(1)))
        }
        "editor" => run(godot.command(&project)?.arg("--editor")),
        "smoke" => {
            run(godot
                .command(&project)?
                .args(["--headless", "--editor", "--import", "--quit"]))?;
            scenario(&godot, &project, "smoke", true)
        }
        "multiplayer" => multiplayer(&repo, &target, &godot),
        "export-debug" | "export-release" => {
            export(&repo, &target, &godot, action == "export-release")
        }
        "streaming" => {
            let output = support::capture(cargo(&repo).args([
                "test",
                "-p",
                "client",
                "--test",
                "multiplayer",
                "--jobs",
                "2",
                "exploration_streams_generated_columns_unloads_and_reconnects_at_negative_position",
                "--",
                "--nocapture",
            ]))?;
            std::io::stdout().write_all(&output)?;
            let text = std::str::from_utf8(&output)?;
            let save = text
                .lines()
                .filter_map(|line| line.strip_prefix("STREAMING_TEST_SAVE="))
                .next_back()
                .ok_or("Streaming test did not provide its isolated save path")?;
            run(godot
                .command(&project)?
                .args(["--script", "res://tests/streaming.gd"])
                .env("CDDA_SAVE_PATH", save))
        }
        action => {
            let (test, headless, fixture) = match action {
                "industry" => ("industry", true, None),
                "industry-graphics" => ("industry", false, None),
                "industry-flow" | "industry-flow-graphics" => (
                    "industry_flow",
                    action == "industry-flow",
                    Some((
                        "client",
                        "industrial_flow_controls_conserve_and_emit_graphics_fixture",
                    )),
                ),
                "industry-thermal" | "industry-thermal-graphics" => (
                    "industry_thermal",
                    action == "industry-thermal",
                    Some((
                        "client",
                        "industrial_thermal_controls_sync_and_emit_graphics_fixture",
                    )),
                ),
                "industry-route" | "industry-route-graphics" => (
                    "industry_route",
                    action == "industry-route",
                    Some((
                        "client",
                        "industrial_pipe_routes_are_atomic_and_emit_graphics_fixture",
                    )),
                ),
                "industry-design" | "industry-design-graphics" => (
                    "industry_design",
                    action == "industry-design",
                    Some((
                        "client",
                        "industrial_parameterized_tanks_and_assembly_emit_graphics_fixture",
                    )),
                ),
                "distillation" => ("distillation", true, None),
                "distillation-graphics" => ("distillation", false, None),
                "column" | "column-graphics" => (
                    "column",
                    action == "column",
                    Some((
                        "client",
                        "column_construction_and_external_connections_emit_graphics_fixture",
                    )),
                ),
                "column-loop" | "column-loop-graphics" => (
                    "column_loop",
                    action == "column-loop",
                    Some((
                        "client",
                        "reflux_equipment_and_complete_loop_emit_graphics_fixture",
                    )),
                ),
                "containers" => (
                    "containers",
                    true,
                    Some((
                        "server",
                        "natural_spawn_is_safe_seeded_and_supports_sixteen_players",
                    )),
                ),
                "structures" => (
                    "structures",
                    false,
                    Some(("server", "structure_sites_emit_fixture")),
                ),
                "ecology" => (
                    "ecology",
                    false,
                    Some((
                        "server",
                        "ecology_sites_cover_materials_and_emit_graphics_fixture",
                    )),
                ),
                "natural-spawn" => ("natural_spawn", false, None),
                "streaming-perf" => ("streaming_performance", false, None),
                "editing" => ("editing", true, None),
                "commands" => ("commands", true, None),
                _ => unreachable!("actions validated before building"),
            };
            if let Some((package, filter)) = fixture {
                let mut command = cargo(&repo);
                command.args(["test", "-p", package, "--jobs", "2"]);
                if package == "client" {
                    command.args([
                        "--test",
                        if matches!(test, "column" | "column_loop") {
                            test
                        } else {
                            "industry"
                        },
                    ]);
                }
                run(command.arg(filter))?;
            }
            scenario(&godot, &project, test, headless)
        }
    }
}

fn scenario(godot: &Godot, project: &Path, test: &str, headless: bool) -> Result {
    let mut command = godot.command(project)?;
    if headless {
        command.arg("--headless");
    }
    run(command.args(["--script", &format!("res://tests/{test}.gd")]))
}

fn multiplayer(repo: &Path, target: &Path, godot: &Godot) -> Result {
    run(cargo(repo).args(["build", "-p", "server", "--jobs", "2"]))?;
    let folder = support::unique_directory(target, "godot-network")?;
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    let address = socket.local_addr()?;
    drop(socket);
    let save = folder.join("world3d.bin");
    let cert = folder.join("server.cert.der");
    let mut command = std::process::Command::new(target.join("debug/server.exe"));
    support::hidden(&mut command)
        .current_dir(repo)
        .env("CDDA_BIND", address.to_string())
        .env("CDDA_SAVE_PATH", &save)
        .env("CDDA_CERT_PATH", &cert)
        .env("CDDA_RUN_SECONDS", "25")
        .env("CDDA_SHUTDOWN_ON_STDIN_CLOSE", "1")
        .stdin(Stdio::piped())
        .stdout(fs::File::create(folder.join("server.out.log"))?)
        .stderr(fs::File::create(folder.join("server.err.log"))?);
    let mut server = Authority(command.spawn()?);
    let start = Instant::now();
    while !cert.is_file() {
        if let Some(status) = server.0.try_wait()? {
            return Err(format!(
                "Test authority failed to start: {status}; {}",
                folder.display()
            )
            .into());
        }
        if start.elapsed() > Duration::from_secs(5) {
            return Err("Test authority certificate timed out".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
    let result = run(godot
        .command(&repo.join("godot"))?
        .args(["--headless", "--script", "res://tests/multiplayer.gd"])
        .env("CDDA_TEST_ADDRESS", address.to_string())
        .env("CDDA_SAVE_PATH", &save)
        .env("CDDA_CERT_PATH", &cert));
    server.stop()?;
    if !save.is_file() {
        return Err("Test authority did not save its isolated world".into());
    }
    println!("Godot network evidence: {}", folder.display());
    result
}

fn export(repo: &Path, target: &Path, godot: &Godot, release: bool) -> Result {
    let templates = std::path::PathBuf::from(
        env::var_os("APPDATA").ok_or("APPDATA is required for Windows export templates")?,
    )
    .join("Godot/export_templates/4.7.2.stable");
    let staged_templates = target.join("godot-templates");
    fs::create_dir_all(&staged_templates)?;
    for name in ["windows_debug_x86_64.exe", "windows_release_x86_64.exe"] {
        fs::copy(templates.join(name), staged_templates.join(name)).map_err(|e| {
            format!(
                "Install Godot 4.7.2 standard export templates ({}): {e}",
                templates.display()
            )
        })?;
    }
    if release {
        support::build_extension(repo, target, true)?;
    }
    let action = if release {
        "export-release"
    } else {
        "export-debug"
    };
    let output = target.join(action);
    fs::create_dir_all(&output)?;
    let stage = support::unique_directory(target, "godot-export-stage")?;
    for item in [
        "project.godot",
        "game.gdextension",
        "game.gdextension.uid",
        "export_presets.cfg",
        "bin",
        "materials",
        "scenes",
        "src",
    ] {
        let source = repo.join("godot").join(item);
        if source.exists() {
            support::copy_tree(&source, &stage.join(item))?;
        }
    }
    let project = stage.join("project.godot");
    fs::write(
        &project,
        support::runtime_project(&fs::read_to_string(&project)?),
    )?;
    let presets = stage.join("export_presets.cfg");
    fs::write(
        &presets,
        fs::read_to_string(&presets)?.replace(
            "../target/godot-templates/",
            &format!("{}/", staged_templates.to_string_lossy().replace('\\', "/")),
        ),
    )?;
    fs::create_dir_all(stage.join(".godot"))?;
    fs::write(
        stage.join(".godot/extension_list.cfg"),
        "res://game.gdextension\n",
    )?;
    run(godot
        .command(&stage)?
        .args(["--headless", "--editor", "--import", "--quit"]))?;
    run(godot
        .command(&stage)?
        .arg("--headless")
        .arg(format!("--{action}"))
        .arg("Windows Desktop")
        .arg(output.join("Dungeon.exe")))?;
    println!("Exported runtime-only project: {}", output.display());
    Ok(())
}
