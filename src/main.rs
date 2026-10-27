//! `vela` command-line client. It links no GTK, so `vela toggle` costs about a
//! millisecond; the GUI lives in the `vela-daemon` binary.

use clap::{Parser, Subcommand};
use std::os::unix::process::CommandExt;
use std::process::ExitCode;
use std::time::{Duration, Instant};
use vela::ipc::{self, Command, SendError};
use vela::{config, idle, launch, paths, shell};

/// Spotlight-style launcher for Hyprland with file search and Claude Code.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand, Clone)]
enum Cmd {
    /// Run the background daemon (normally started by systemd).
    Daemon {
        /// Show the launcher right after start.
        #[arg(long)]
        show: bool,
    },
    /// Show or hide the launcher (default). Starts the daemon if needed.
    Toggle,
    /// Show the launcher.
    Show,
    /// Hide the launcher.
    Hide,
    /// Open the settings window, optionally on a page
    /// (general, apps, search, claude, panel, notifications, power, appearance, pulse, updates, system).
    Settings {
        #[arg(value_parser = clap::builder::PossibleValuesParser::new(ipc::SETTINGS_PAGES))]
        page: Option<String>,
    },
    /// Reload configuration, applications and the file index.
    Reload,
    /// Stop the daemon.
    Quit,
    /// Exit successfully if the daemon is running.
    Status,
    /// Print the default configuration (TOML).
    DefaultConfig,
    /// Change one setting, e.g. `vela set idle.suspend false`.
    Set { path: String, value: String },
    /// Run the control center (Quickshell, `qs`) with vela's settings.
    Shell,
    /// Toggle, open or close the control center.
    Panel {
        #[arg(default_value = "toggle", value_parser = ["toggle", "open", "close"])]
        action: String,
    },
    /// Run hypridle with the [idle] settings; restarts it when they change.
    Idle,
    /// Print the Claude plan usage of every Claude Code profile as JSON.
    ClaudeUsage {
        /// Only the names of the shown profiles that are logged in (no request).
        #[arg(long)]
        list: bool,
        /// Print the background poller's results whenever they change (used
        /// by the panel; no requests of its own).
        #[arg(long)]
        watch: bool,
    },
    /// MCP server on stdin/stdout: lets Claude change Hyprland and vela
    /// settings (`claude mcp add vela -- vela mcp`).
    Mcp,
    /// Screen-share picker for xdg-desktop-portal-hyprland; prints the
    /// selection for xdph (`vela-share-picker` is the same for its config).
    SharePicker {
        /// Preselect "remember this choice" (xdph: allow_token_by_default).
        #[arg(long)]
        allow_token: bool,
    },
    /// System updates: `check` asks the daemon to look for updates now,
    /// `list` prints what is pending as JSON, `watch` prints the daemon's
    /// update state whenever it changes (used by the control center).
    Updates {
        #[arg(default_value = "list", value_parser = ["check", "list", "watch"])]
        action: String,
    },
    /// Pulse, the task manager: opens its window (default), or `serve`
    /// (the window's backend, JSON lines), `snapshot` (one frame as JSON),
    /// `doctor` (what's wrong, in words).
    Pulse {
        #[arg(default_value = "open", value_parser = ["open", "serve", "snapshot", "doctor"])]
        action: String,
    },
    /// Quick Share with phones nearby: `serve` is the control center's
    /// backend (JSON lines on stdin/stdout, logs on stderr).
    Quickshare {
        #[arg(value_parser = ["serve", "send"])]
        action: String,
        /// Files to send (`send`).
        files: Vec<std::path::PathBuf>,
    },
    /// Print which parts of vela are installed (chosen in install.sh).
    Components,
    /// Print the control center settings as JSON (used by the shell).
    ShellConfig {
        /// Keep running and print a new line whenever the config changes.
        #[arg(long)]
        watch: bool,
    },
}

fn main() -> ExitCode {
    paths::complete_nixos_env();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("vela=info"))
        .format_timestamp_millis()
        .init();
    let cli = Cli::parse();
    let cmd = cli.command.unwrap_or(Cmd::Toggle);
    let ipc_cmd = match cmd.clone() {
        Cmd::Daemon { show } => {
            let mut cmd = std::process::Command::new(daemon_binary());
            if show {
                cmd.arg("--show");
            }
            let err = cmd.exec();
            eprintln!("vela: cannot start vela-daemon: {err}");
            return ExitCode::FAILURE;
        }
        Cmd::Toggle => Command::Toggle,
        Cmd::Show => Command::Show,
        Cmd::Hide => Command::Hide,
        Cmd::Settings { page } => Command::Settings(page.and_then(|p| ipc::SETTINGS_PAGES.iter().copied().find(|x| *x == p))),
        Cmd::Reload => Command::Reload,
        Cmd::Quit => Command::Quit,
        Cmd::Status => Command::Ping,
        Cmd::DefaultConfig => {
            print!("{}", config::Config::default().to_toml());
            return ExitCode::SUCCESS;
        }
        Cmd::Set { path, value } => {
            let file = paths::config_file();
            let result = config::load_or_create(&file).and_then(|outcome| {
                let mut cfg = match outcome {
                    config::LoadOutcome::Loaded(c) | config::LoadOutcome::Created(c) => c,
                };
                cfg.set_value(&path, &value)?;
                config::save(&file, &cfg)
            });
            return match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("vela: {e:#}");
                    ExitCode::FAILURE
                }
            };
        }
        Cmd::Mcp => {
            return match vela::mcp::run() {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("vela mcp: {e:#}");
                    ExitCode::FAILURE
                }
            };
        }
        Cmd::Pulse { action } => return pulse(&action),
        Cmd::Quickshare { action, files } => return quickshare(&action, &files),
        Cmd::Components => {
            print_components();
            return ExitCode::SUCCESS;
        }
        Cmd::Shell => return run_qs(&[]),
        Cmd::Panel { action } => return run_qs(&["ipc", "call", "panel", &action]),
        Cmd::ShellConfig { watch } => return shell_config(watch),
        Cmd::Idle => return run_idle(),
        Cmd::SharePicker { allow_token } => {
            let exe = std::env::current_exe().unwrap_or_else(|_| "vela".into());
            return share_picker(&exe, allow_token);
        }
        Cmd::ClaudeUsage { watch: true, .. } => return watch_json(&vela::claude_usage::cache_file()),
        Cmd::Updates { action } => match action.as_str() {
            "check" => Command::UpdateCheck,
            "watch" => return watch_json(&vela::update::status_file()),
            _ => {
                let cfg = std::fs::read_to_string(paths::config_file())
                    .ok()
                    .and_then(|t| config::Config::from_toml(&t).ok())
                    .unwrap_or_default();
                let (pending, errors) = vela::update::check(&vela::update::Tools::detect(&cfg.updates));
                println!("{}", serde_json::json!({ "pending": pending, "errors": errors }));
                return if errors.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE };
            }
        },
        Cmd::ClaudeUsage { list, .. } => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64);
            // A broken config shouldn't hide the numbers: defaults then.
            let cfg = std::fs::read_to_string(paths::config_file())
                .ok()
                .and_then(|t| config::Config::from_toml(&t).ok())
                .unwrap_or_default();
            if list {
                println!("{}", serde_json::json!(vela::claude_usage::usable_names(&paths::home_dir(), &cfg.panel, now)));
            } else {
                println!("{}", vela::claude_usage::report(&paths::home_dir(), &cfg.panel, now));
            }
            return ExitCode::SUCCESS;
        }
    };
    client(ipc_cmd)
}

fn print_components() {
    use std::io::Write;
    use vela::components::{self, Component};
    let installed = components::installed();
    // A closed pipe (`vela components | head -1`) is no error.
    let mut out = std::io::stdout().lock();
    let _ = match components::manifest_file() {
        Some(f) => writeln!(out, "profile: {} ({})", installed.profile.as_deref().unwrap_or("?"), f.display()),
        None if vela::nixos::running_nixos() => writeln!(out, "profile: NixOS default, everything but updates (no programs.vela.components)"),
        None => writeln!(out, "profile: full (no selection from install.sh)"),
    };
    for c in Component::ALL {
        let _ = writeln!(out, "  [{}] {:<13} {}", if installed.has(c) { 'x' } else { ' ' }, c.id(), c.description());
    }
}

fn client(cmd: Command) -> ExitCode {
    let socket = paths::socket_path();
    match ipc::send(&socket, cmd) {
        Ok(reply) if reply == "ok" => ExitCode::SUCCESS,
        Ok(reply) => {
            eprintln!("vela: {reply}");
            ExitCode::FAILURE
        }
        Err(SendError::NotRunning) => {
            if matches!(cmd, Command::Hide | Command::Quit | Command::Reload | Command::Ping) {
                if cmd == Command::Ping {
                    eprintln!("vela: daemon is not running");
                    return ExitCode::FAILURE;
                }
                return ExitCode::SUCCESS;
            }
            match start_daemon() {
                Ok(()) => {
                    let first = if cmd == Command::Toggle { Command::Show } else { cmd };
                    match ipc::send(&socket, first) {
                        Ok(_) => ExitCode::SUCCESS,
                        Err(_) => {
                            eprintln!("vela: daemon started but does not answer");
                            ExitCode::FAILURE
                        }
                    }
                }
                Err(e) => {
                    eprintln!("vela: could not start the daemon: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(SendError::Other(e)) => {
            eprintln!("vela: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Starts the daemon via systemd if the unit is installed, otherwise directly.
fn start_daemon() -> Result<(), String> {
    let unit_installed = ["/usr/lib/systemd/user/vela.service", "/etc/systemd/user/vela.service"]
        .iter()
        .map(std::path::PathBuf::from)
        .chain([
            paths::home_dir().join(".config/systemd/user/vela.service"),
            paths::home_dir().join(".local/share/systemd/user/vela.service"),
        ])
        .any(|p| p.exists());
    let started_by_systemd = unit_installed
        && std::process::Command::new("systemctl")
            .args(["--user", "start", "vela.service"])
            .status()
            .is_ok_and(|s| s.success());
    if !started_by_systemd {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let spec = launch::SpawnSpec {
            argv: vec![exe.to_string_lossy().into_owned(), "daemon".into()],
            name: "vela".into(),
            ..Default::default()
        };
        launch::spawn_detached(&spec, false).map_err(|e| e.to_string())?;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if ipc::send(&paths::socket_path(), Command::Ping).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err("timed out waiting for the daemon".into())
}

/// Execs `qs -p <shell dir> <args>`; the shell finds this binary via VELA_BIN.
fn run_qs(args: &[&str]) -> ExitCode {
    let exe = std::env::current_exe().ok();
    let Some(dir) = shell::find_shell_dir(std::env::var_os("VELA_SHELL_DIR").as_deref(), &shell::shell_dir_candidates(exe.as_deref())) else {
        eprintln!("vela: control center files not found (set VELA_SHELL_DIR or reinstall vela)");
        return ExitCode::FAILURE;
    };
    let Some(qs) = paths::find_executable("qs").or_else(|| paths::find_executable("quickshell")) else {
        eprintln!("vela: Quickshell (qs) is not installed");
        return ExitCode::FAILURE;
    };
    let mut cmd = std::process::Command::new(qs);
    cmd.arg("-p").arg(&dir).args(args);
    if let Some(exe) = exe {
        cmd.env("VELA_BIN", exe);
    }
    let err = cmd.exec();
    eprintln!("vela: cannot start Quickshell: {err}");
    ExitCode::FAILURE
}

/// For long-running subcommands: optionally die with the parent, and notice
/// when an update replaced this binary.
struct Lifecycle {
    exe: Option<std::path::PathBuf>,
    stamp: Option<std::time::SystemTime>,
}

impl Lifecycle {
    /// `with_parent`: for helpers the shell spawns directly; not for `idle`,
    /// which is started detached on purpose.
    fn start(with_parent: bool) -> Lifecycle {
        if with_parent {
            let parent = unsafe { libc::getppid() };
            // SAFETY: plain prctl/getppid calls without pointers.
            unsafe {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            }
            // The parent died before prctl took effect.
            if unsafe { libc::getppid() } != parent {
                std::process::exit(0);
            }
        }
        // Remember the path now: after an update /proc/self/exe reads "… (deleted)".
        let exe = std::env::current_exe().ok().filter(|p| !p.to_string_lossy().ends_with(" (deleted)"));
        let stamp = exe.as_ref().and_then(|p| p.metadata().ok()?.modified().ok());
        Lifecycle { exe, stamp }
    }

    /// Re-execs the new binary with the same arguments after an update.
    fn reexec_if_updated(&self, before: impl FnOnce()) {
        let Some(exe) = &self.exe else { return };
        let now = exe.metadata().ok().and_then(|m| m.modified().ok());
        if now.is_none() || now == self.stamp {
            return;
        }
        before();
        let err = std::process::Command::new(exe).args(std::env::args_os().skip(1)).exec();
        eprintln!("vela: cannot restart after update: {err}");
    }
}

/// Prints a JSON file (one line) whenever it changes.
fn watch_json(file: &std::path::Path) -> ExitCode {
    use std::io::Write;
    let life = Lifecycle::start(true);
    let mut stamp = None;
    let mut out = std::io::stdout();
    loop {
        let now = std::fs::metadata(file).ok().and_then(|m| m.modified().ok());
        if now.is_some() && now != stamp {
            stamp = now;
            // Re-serialized: the panel reads one JSON document per line.
            if let Some(line) = std::fs::read_to_string(file)
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                && writeln!(out, "{line}").and_then(|()| out.flush()).is_err()
            {
                return ExitCode::SUCCESS;
            }
        }
        life.reexec_if_updated(|| {});
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn shell_config(watch: bool) -> ExitCode {
    use std::io::Write;
    let life = watch.then(|| Lifecycle::start(true));
    let mut watcher = shell::ConfigWatcher::new(paths::config_file());
    let mut out = std::io::stdout();
    loop {
        if let Some(line) = watcher.poll() {
            // The reading shell went away: stop.
            if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
                return ExitCode::SUCCESS;
            }
        }
        if let Some(e) = watcher.take_error() {
            eprintln!("vela: {e}");
            if !watch {
                return ExitCode::FAILURE;
            }
        }
        if !watch {
            return ExitCode::SUCCESS;
        }
        if let Some(life) = &life {
            life.reexec_if_updated(|| {});
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn pulse(action: &str) -> ExitCode {
    match action {
        "serve" => {
            let _life = Lifecycle::start(true);
            match vela::pulse::serve::run() {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("vela: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        "snapshot" | "doctor" => {
            let mut m = vela::pulse::engine::Monitor::new();
            m.tick(false);
            std::thread::sleep(Duration::from_millis(700));
            let f = m.tick(action == "snapshot");
            if action == "snapshot" {
                println!("{}", serde_json::to_string(&f).unwrap_or_default());
            } else {
                print!("{}", vela::pulse::doctor::report(&f.sample, &f.apps, &f.findings, f.score));
            }
            ExitCode::SUCCESS
        }
        _ => open_pulse(),
    }
}

fn quickshare(action: &str, files: &[std::path::PathBuf]) -> ExitCode {
    match action {
        "serve" => {
            let _life = Lifecycle::start(true);
            match vela::quickshare::serve::run() {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("vela: {e:#}");
                    ExitCode::FAILURE
                }
            }
        }
        // The running control center takes the files (its IPC handler opens the
        // Send tab); `qs ipc` fails when no shell runs.
        _ => match vela::quickshare::files::send_json(files) {
            Ok(json) => run_qs(&["ipc", "call", "quickshare", "send", &json]),
            Err(e) => {
                eprintln!("vela: quickshare send: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

/// Shows the running Pulse window or starts one (its own Quickshell instance).
fn open_pulse() -> ExitCode {
    let exe = std::env::current_exe().ok();
    let Some(dir) = shell::find_shell_dir(std::env::var_os("VELA_SHELL_DIR").as_deref(), &shell::shell_dir_candidates(exe.as_deref())) else {
        eprintln!("vela: Pulse files not found (set VELA_SHELL_DIR or reinstall vela)");
        return ExitCode::FAILURE;
    };
    let Some(qs) = paths::find_executable("qs").or_else(|| paths::find_executable("quickshell")) else {
        eprintln!("vela: Quickshell (qs) is not installed");
        return ExitCode::FAILURE;
    };
    let entry = dir.join("pulse.qml");
    let ipc = |args: &[&str]| {
        std::process::Command::new(&qs)
            .arg("-p")
            .arg(&entry)
            .arg("ipc")
            .args(args)
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };
    // Not `show`: `qs ipc call pulse show` is read as `qs ipc show` (list).
    if ipc(&["call", "pulse", "present"]).as_deref() == Some("true") {
        return ExitCode::SUCCESS;
    }
    // Running without a window (closed by an older Pulse, a reload that lost
    // it) or too old to say: end it and start a fresh one.
    if ipc(&["show"]).is_some() {
        ipc(&["call", "pulse", "quit"]);
        for _ in 0..30 {
            if ipc(&["show"]).is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let mut cmd = std::process::Command::new(qs);
    cmd.arg("-p").arg(&entry).arg("-d");
    if let Some(exe) = exe {
        cmd.env("VELA_BIN", exe);
    }
    match cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(s) => {
            eprintln!("vela: Pulse exited with {s}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("vela: cannot start Quickshell: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Keeps hypridle running with a config generated from `[idle]`.
fn run_idle() -> ExitCode {
    use std::os::unix::process::CommandExt as _;
    let Some(hypridle) = paths::find_executable("hypridle") else {
        eprintln!("vela: hypridle is not installed");
        return ExitCode::FAILURE;
    };
    let life = Lifecycle::start(false);
    let conf_path = paths::state_dir().join("hypridle.conf");
    let kbd = idle::keyboard_backlight();
    let mut current: Option<config::Idle> = None;
    let mut child: Option<std::process::Child> = None;
    let mut stamp = None;
    loop {
        let meta = std::fs::metadata(paths::config_file()).ok();
        let now = meta.map(|m| (m.modified().ok(), m.len()));
        if now != stamp || current.is_none() {
            stamp = now;
            let wanted = match std::fs::read_to_string(paths::config_file()) {
                Ok(text) => config::Config::from_toml(&text).map(|c| c.idle).ok(),
                Err(_) => Some(config::Idle::default()),
            };
            // A broken file keeps the running settings.
            if let Some(wanted) = wanted.filter(|w| current.as_ref() != Some(w)) {
                if let Err(e) = config::write_atomic(&conf_path, &idle::hypridle_conf(&wanted, kbd.as_deref())) {
                    eprintln!("vela: cannot write {}: {e:#}", conf_path.display());
                    return ExitCode::FAILURE;
                }
                if let Some(mut c) = child.take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                current = Some(wanted);
            }
        }
        // (Re)start hypridle: after a change, or if it died.
        let running = child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)));
        if !running {
            let mut cmd = std::process::Command::new(&hypridle);
            cmd.arg("-q").arg("-c").arg(&conf_path);
            // SAFETY: prctl is async-signal-safe; hypridle dies with us.
            unsafe {
                cmd.pre_exec(|| {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                    Ok(())
                });
            }
            match cmd.spawn() {
                Ok(c) => child = Some(c),
                Err(e) => eprintln!("vela: cannot start hypridle: {e}"),
            }
        }
        life.reexec_if_updated(|| {
            // The new image starts its own hypridle.
            if let Some(mut c) = child.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        });
        std::thread::sleep(Duration::from_millis(1000));
    }
}

fn share_picker(vela_bin: &std::path::Path, allow_token: bool) -> ExitCode {
    match vela::sharepick::run(vela_bin, allow_token) {
        Ok(Some(line)) => {
            print!("{line}");
            ExitCode::SUCCESS
        }
        Ok(None) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("vela share-picker: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `vela-daemon` next to this binary, or from PATH.
fn daemon_binary() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("vela-daemon")))
        .filter(|p| p.exists())
        .or_else(|| paths::find_executable("vela-daemon"))
        .unwrap_or_else(|| "vela-daemon".into())
}
