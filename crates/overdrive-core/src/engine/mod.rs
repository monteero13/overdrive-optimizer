//! Module D — Transaction Engine (goals.md §5.7, docs/architecture/transaction-engine.md).
//!
//! - [`Action`]: closed set of serialisable primitives. Each one knows how to capture the
//!   original state, apply, verify, revert to exactly what was captured, and verify that.
//! - [`SystemOps`]: the system port (real one in `overdrive-win`, fake in [`fake`]).
//! - [`journal::Journal`]: write-ahead journal on SQLite (schema = docs/architecture/journal.sql).
//! - [`tx::Engine`]: apply / revert / recover / revert_all, with injectable crash points.

pub mod actions;
pub mod fake;
pub mod journal;
pub mod tx;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "hive", content = "sid")]
pub enum RegRoot {
    LocalMachine,
    CurrentUser,
    /// HKEY_USERS\<SID> — how the LocalSystem service reaches the interactive user's settings.
    User(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegData {
    Dword(u32),
    String(String),
}

/// Desired registry state: a value, or "this value must not exist".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegTarget {
    Set(RegData),
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartType {
    Boot,
    System,
    Automatic,
    Manual,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorityClass {
    Idle,
    BelowNormal,
    Normal,
    AboveNormal,
    High,
    Realtime,
}

/// A process instance. `start_time` (FILETIME) makes it immune to PID reuse: the real
/// implementation refuses to act if the PID now belongs to a different process.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcRef {
    pub pid: u32,
    pub start_time: u64,
    pub image: String,
}

/// The closed set of things the engine knows how to change.
/// (Scheduled tasks, driver profiles and file patches arrive with Phases 6–7.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Action {
    /// Also covers IFEO PerfOptions, UserGpuPreferences, Game DVR, … (all registry values).
    RegistryValue {
        root: RegRoot,
        key: String,
        name: String,
        target: RegTarget,
    },
    ServiceStart {
        name: String,
        start: StartType,
    },
    PowerScheme {
        guid: String,
    },
    ProcessPriority {
        process: ProcRef,
        class: PriorityClass,
    },
    ProcessEcoQos {
        process: ProcRef,
        on: bool,
    },
    ProcessSuspend {
        process: ProcRef,
    },
    /// Enable/disable a Task Scheduler task, e.g. `\Microsoft\Windows\Application Experience\ProgramDataUpdater`.
    ScheduledTask {
        path: String,
        enabled: bool,
    },
}

impl Action {
    /// Matches `steps.action_kind` in the journal schema.
    pub fn kind(&self) -> &'static str {
        match self {
            Action::RegistryValue { .. } => "registry_value",
            Action::ServiceStart { .. } => "service_start",
            Action::PowerScheme { .. } => "power_scheme",
            Action::ScheduledTask { .. } => "scheduled_task",
            Action::ProcessPriority { .. } => "process_priority",
            Action::ProcessEcoQos { .. } => "process_eco_qos",
            Action::ProcessSuspend { .. } => "process_suspend",
        }
    }

    pub fn process(&self) -> Option<&ProcRef> {
        match self {
            Action::ProcessPriority { process, .. }
            | Action::ProcessEcoQos { process, .. }
            | Action::ProcessSuspend { process } => Some(process),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum SysError {
    #[error("access denied: {0}")]
    AccessDenied(String),
    #[error("not found: {0}")]
    NotFound(String),
    /// The PID exists but belongs to a different process than the journaled one.
    #[error("process identity mismatch for pid {0}")]
    ProcessMismatch(u32),
    #[error("{0}")]
    Other(String),
}

pub type SysResult<T> = Result<T, SysError>;

/// System port. Every call is short and idempotent where it can be.
/// Process methods must verify `ProcRef::start_time` before touching anything.
pub trait SystemOps {
    fn reg_read(&self, root: &RegRoot, key: &str, name: &str) -> SysResult<Option<RegData>>;
    fn reg_write(&self, root: &RegRoot, key: &str, name: &str, data: &RegData) -> SysResult<()>;
    /// Deleting a value that doesn't exist is success.
    fn reg_delete(&self, root: &RegRoot, key: &str, name: &str) -> SysResult<()>;
    fn reg_key_exists(&self, root: &RegRoot, key: &str) -> SysResult<bool>;
    /// Removes the key only if it has no values and no subkeys (never deletes user data).
    fn reg_delete_key_if_empty(&self, root: &RegRoot, key: &str) -> SysResult<()>;

    fn service_start_type(&self, name: &str) -> SysResult<StartType>;
    fn set_service_start_type(&self, name: &str, start: StartType) -> SysResult<()>;

    fn active_power_scheme(&self) -> SysResult<String>;
    fn set_power_scheme(&self, guid: &str) -> SysResult<()>;
    /// GUIDs (lowercase) of the power schemes that exist on this machine.
    fn power_schemes(&self) -> SysResult<Vec<String>>;

    /// `NotFound` if the task doesn't exist on this Windows build.
    fn task_enabled(&self, path: &str) -> SysResult<bool>;
    fn set_task_enabled(&self, path: &str, enabled: bool) -> SysResult<()>;

    /// `false` if the process exited (or the PID was reused).
    fn proc_alive(&self, p: &ProcRef) -> SysResult<bool>;
    fn proc_priority(&self, p: &ProcRef) -> SysResult<PriorityClass>;
    fn set_proc_priority(&self, p: &ProcRef, c: PriorityClass) -> SysResult<()>;
    /// `None` = this Windows build cannot report it (Windows 10 can set EcoQoS but not query it).
    fn proc_eco_qos(&self, p: &ProcRef) -> SysResult<Option<bool>>;
    fn set_proc_eco_qos(&self, p: &ProcRef, on: bool) -> SysResult<()>;
    fn suspend(&self, p: &ProcRef) -> SysResult<()>;
    fn resume(&self, p: &ProcRef) -> SysResult<()>;
}
