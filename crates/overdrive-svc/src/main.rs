//! overdrive-svc
//!
//!   run                      entry point used by the Service Control Manager
//!   install | uninstall      register / remove the service (admin). Uninstall reverts
//!                            every change Overdrive made before removing itself.
//!   console [--live] [--data DIR] [--pipe NAME]
//!                            run in this terminal. DRY-RUN by default: real reads,
//!                            simulated writes; nothing on the PC changes. `--live`
//!                            applies changes for real (as the current user).

use std::path::PathBuf;

use overdrive_svc::core::{Config, Core};
use overdrive_svc::overlay::OverlaySystem;
use overdrive_svc::server::{self, ClientPolicy};
use overdrive_svc::{service, win};
use overdrive_win::system_ops::WinSystem;

fn arg(args: &[String], k: &str) -> Option<String> {
    args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => {
            if let Err(e) = service::dispatch() {
                eprintln!("not started by the service manager: {e}");
                std::process::exit(1);
            }
        }
        Some("install") => exit(service::install().map(|_| "installed and started".to_string())),
        Some("uninstall") => exit(service::uninstall()),
        Some("console") => console(&args),
        _ => {
            eprintln!("usage: overdrive-svc run | install | uninstall | console [--live] [--data DIR] [--pipe NAME]");
            std::process::exit(2);
        }
    }
}

fn exit(r: Result<String, String>) {
    match r {
        Ok(m) => println!("{m}"),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

fn console(args: &[String]) {
    let live = args.iter().any(|a| a == "--live");
    let base = std::env::var("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."));
    let data = arg(args, "--data")
        .map(PathBuf::from)
        .unwrap_or_else(|| base.join("Overdrive").join(if live { "dev-live" } else { "dev-dry-run" }));
    let pipe = arg(args, "--pipe").unwrap_or_else(|| overdrive_ipc::PIPE_NAME.to_string());
    let cfg = Config { data_dir: data.clone(), svc_version: env!("CARGO_PKG_VERSION").into(), allow_reboot: false };
    let sys: Box<dyn overdrive_core::engine::SystemOps> =
        if live { Box::new(WinSystem) } else { Box::new(OverlaySystem::new(WinSystem)) };
    eprintln!(
        "overdrive-svc console · {} · data {} · pipe {pipe}",
        if live { "LIVE (changes are real)" } else { "dry-run (nothing on this PC changes)" },
        data.display()
    );
    let core = match Core::new(cfg, sys, Box::new(win::WinPlatform)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let (_tx, rx) = match server::listen(&pipe, overdrive_ipc::DEFAULT_SDDL, ClientPolicy::Any) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("pipe {pipe}: {e} (is the installed service already running?)");
            std::process::exit(1);
        }
    };
    eprintln!("ready");
    server::run(core, rx);
}
