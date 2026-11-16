//! Windows service plumbing: SCM entry point, install and uninstall.
//!
//! Uninstall puts the PC back first: every change still recorded in the journal is
//! reverted (and verified) before the service is deleted.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::Duration;

use overdrive_core::engine::journal::Journal;
use overdrive_core::engine::tx::Engine;
use overdrive_win::system_ops::WinSystem;
use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
    ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

use crate::core::{Config, Core};
use crate::server::{self, ClientPolicy, Msg};

pub const SERVICE_NAME: &str = "OverdriveSvc";
pub const DISPLAY_NAME: &str = "Overdrive";
const DESCRIPTION: &str = "Overdrive: optimizaciones de juego medidas y reversibles. Al desinstalar, deshace todos sus cambios.";

/// `%ProgramData%\Overdrive`, readable and writable only by SYSTEM and Administrators:
/// a standard user must never be able to edit the journal the service acts on.
pub fn data_dir() -> PathBuf {
    let pd = std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into());
    PathBuf::from(pd).join("Overdrive")
}

pub fn lock_data_dir(dir: &PathBuf) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let st = std::process::Command::new("icacls.exe")
        .arg(dir)
        .args(["/inheritance:r", "/grant:r", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F", "/Q"])
        .status()?;
    if st.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("icacls exited with {st}")))
    }
}

static STOP: Mutex<Option<Sender<Msg>>> = Mutex::new(None);

define_windows_service!(ffi_service_main, service_main);

pub fn dispatch() -> windows_service::Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}

fn status(state: ServiceState, accept: ServiceControlAccept, code: u32) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: accept,
        exit_code: ServiceExitCode::Win32(code),
        checkpoint: 0,
        wait_hint: Duration::from_secs(20),
        process_id: None,
    }
}

fn service_main(_args: Vec<OsString>) {
    let handler = service_control_handler::register(SERVICE_NAME, |c| match c {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            if let Some(tx) = STOP.lock().unwrap().as_ref() {
                let _ = tx.send(Msg::Stop);
            }
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    });
    let Ok(handle) = handler else { return };
    let _ = handle.set_service_status(status(ServiceState::StartPending, ServiceControlAccept::empty(), 0));
    let dir = data_dir();
    let _ = lock_data_dir(&dir);
    let code = match start(dir) {
        Ok((core, tx, rx)) => {
            *STOP.lock().unwrap() = Some(tx);
            let _ = handle.set_service_status(status(
                ServiceState::Running,
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
                0,
            ));
            server::run(core, rx);
            0
        }
        Err(e) => {
            let _ = std::fs::write(data_dir().join("startup-error.txt"), &e);
            1
        }
    };
    let _ = handle.set_service_status(status(ServiceState::Stopped, ServiceControlAccept::empty(), code));
}

type Started = (Core, Sender<Msg>, std::sync::mpsc::Receiver<Msg>);

fn start(dir: PathBuf) -> Result<Started, String> {
    let core = Core::new(
        Config {
            data_dir: dir,
            svc_version: env!("CARGO_PKG_VERSION").into(),
            allow_reboot: true,
        },
        Box::new(WinSystem),
        Box::new(crate::win::WinPlatform),
    )?;
    let (tx, rx) = server::listen(overdrive_ipc::PIPE_NAME, overdrive_ipc::DEFAULT_SDDL, ClientPolicy::InstallDirOnly)
        .map_err(|e| format!("pipe: {e}"))?;
    Ok((core, tx, rx))
}

pub fn install() -> Result<(), String> {
    let m = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)
        .map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // Re-install = update: stop and delete the old registration first.
    if let Ok(old) = m.open_service(SERVICE_NAME, ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS) {
        let _ = old.stop();
        wait_stopped(&old);
        let _ = old.delete();
        drop(old);
        std::thread::sleep(Duration::from_millis(500));
    }
    let info = ServiceInfo {
        name: SERVICE_NAME.into(),
        display_name: DISPLAY_NAME.into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: vec!["run".into()],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let s = m
        .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
        .map_err(|e| e.to_string())?;
    let _ = s.set_description(DESCRIPTION);
    let _ = s.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86_400)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![
            ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(5) },
            ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(30) },
            ServiceAction { action_type: ServiceActionType::None, delay: Duration::ZERO },
        ]),
    });
    lock_data_dir(&data_dir()).map_err(|e| e.to_string())?;
    s.start::<&str>(&[]).map_err(|e| e.to_string())?;
    Ok(())
}

fn wait_stopped(s: &windows_service::service::Service) {
    for _ in 0..100 {
        match s.query_status() {
            Ok(st) if st.current_state != ServiceState::Stopped => std::thread::sleep(Duration::from_millis(200)),
            _ => return,
        }
    }
}

/// Stop the service, revert everything Overdrive ever applied, delete the service.
pub fn uninstall() -> Result<String, String> {
    let m = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).map_err(|e| e.to_string())?;
    if let Ok(s) = m.open_service(SERVICE_NAME, ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS) {
        let _ = s.stop();
        wait_stopped(&s);
        s.delete().map_err(|e| e.to_string())?;
    }
    let dir = data_dir();
    let db = dir.join("journal.db");
    if !db.exists() {
        return Ok("nothing to revert".into());
    }
    let j = Journal::open(&db, env!("CARGO_PKG_VERSION")).map_err(|e| e.to_string())?;
    let rep = Engine::new(&j, &WinSystem).revert_all().map_err(|e| e.to_string())?;
    drop(j);
    if rep.remaining_diffs.is_empty() {
        let _ = std::fs::remove_dir_all(&dir);
        Ok(format!("reverted {} transactions; data removed", rep.reverted.len()))
    } else {
        Ok(format!(
            "reverted {} transactions; still different (journal kept in {}): {}",
            rep.reverted.len(),
            dir.display(),
            rep.remaining_diffs.join(", ")
        ))
    }
}
