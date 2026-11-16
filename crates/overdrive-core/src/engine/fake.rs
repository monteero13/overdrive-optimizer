//! In-memory `SystemOps` with failure injection and a log of every process "handle"
//! opened — used by the engine tests (crash at every step, safety guard, rollback).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::{
    DriverSetting, FileData, PriorityClass, ProcRef, RegData, RegRoot, StartType, SysError,
    SysResult, SystemOps,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeProc {
    pub start_time: u64,
    pub image: String,
    pub priority: PriorityClass,
    pub eco_qos: bool,
    pub suspended: bool,
    pub alive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FakeState {
    pub reg: BTreeMap<(String, String, String), RegData>,
    /// Existing registry keys (root, lowercase path) — keys can exist without values.
    pub reg_keys: BTreeSet<(String, String)>,
    pub services: BTreeMap<String, StartType>,
    pub power: String,
    /// Power schemes that exist (empty = only the active one).
    pub power_schemes: BTreeSet<String>,
    pub procs: BTreeMap<u32, FakeProc>,
    pub tasks: BTreeMap<String, bool>,
    /// Files by lowercase path: (bytes, read-only).
    pub files: BTreeMap<String, (Vec<u8>, bool)>,
    /// Per-exe driver user values (lowercase exe).
    pub driver: BTreeMap<(String, DriverSetting), u32>,
    /// The machine has a driver with per-exe profiles (NVIDIA).
    pub driver_profiles: bool,
}

impl FakeState {
    /// Insert a value and all its ancestor keys (like the real registry would have).
    pub fn add_reg(&mut self, root: &RegRoot, key: &str, name: &str, data: RegData) {
        add_key_chain(&mut self.reg_keys, root, key);
        self.reg.insert(rk(root, key, name), data);
    }
}

fn add_key_chain(keys: &mut BTreeSet<(String, String)>, root: &RegRoot, key: &str) {
    let segs: Vec<String> = key
        .to_ascii_lowercase()
        .split('\\')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    for i in 1..=segs.len() {
        keys.insert((format!("{root:?}"), segs[..i].join("\\")));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The n-th (0-based) call to `op` returns an error.
    FailNth(&'static str, usize),
    /// Every call to `op` returns an error.
    FailAlways(&'static str),
    /// Calls to `op` return Ok but change nothing (→ verification must catch it).
    Silent(&'static str),
}

#[derive(Default)]
pub struct FakeSystem {
    pub state: RefCell<FakeState>,
    pub faults: RefCell<Vec<Fault>>,
    calls: RefCell<HashMap<&'static str, usize>>,
    /// Every pid a process method touched (the real impl would open a handle).
    pub opened: RefCell<Vec<u32>>,
}

fn rk(root: &RegRoot, key: &str, name: &str) -> (String, String, String) {
    (
        format!("{root:?}"),
        key.to_ascii_lowercase(),
        name.to_ascii_lowercase(),
    )
}

impl FakeSystem {
    pub fn new(state: FakeState) -> Self {
        Self {
            state: RefCell::new(state),
            ..Default::default()
        }
    }

    pub fn snapshot(&self) -> FakeState {
        self.state.borrow().clone()
    }

    pub fn fault(&self, f: Fault) {
        self.faults.borrow_mut().push(f);
    }

    /// Returns Err for injected failures, Ok(true) if the call should actually mutate.
    fn enter(&self, op: &'static str) -> SysResult<bool> {
        let n = {
            let mut c = self.calls.borrow_mut();
            let e = c.entry(op).or_insert(0);
            *e += 1;
            *e - 1
        };
        for f in self.faults.borrow().iter() {
            match *f {
                Fault::FailNth(o, k) if o == op && k == n => {
                    return Err(SysError::Other(format!("injected failure in {op} #{n}")))
                }
                Fault::FailAlways(o) if o == op => {
                    return Err(SysError::Other(format!("injected failure in {op}")))
                }
                Fault::Silent(o) if o == op => return Ok(false),
                _ => {}
            }
        }
        Ok(true)
    }

    fn proc<'a>(&self, st: &'a mut FakeState, p: &ProcRef) -> SysResult<Option<&'a mut FakeProc>> {
        self.opened.borrow_mut().push(p.pid);
        match st.procs.get_mut(&p.pid) {
            Some(fp) if fp.start_time != p.start_time => Err(SysError::ProcessMismatch(p.pid)),
            Some(fp) if fp.alive => Ok(Some(fp)),
            _ => Ok(None),
        }
    }

    fn with_proc<T>(
        &self,
        op: &'static str,
        p: &ProcRef,
        f: impl FnOnce(&mut FakeProc, bool) -> T,
    ) -> SysResult<T> {
        let mutate = self.enter(op)?;
        let mut st = self.state.borrow_mut();
        let fp = self
            .proc(&mut st, p)?
            .ok_or_else(|| SysError::NotFound(format!("pid {}", p.pid)))?;
        Ok(f(fp, mutate))
    }
}

impl SystemOps for FakeSystem {
    fn reg_read(&self, root: &RegRoot, key: &str, name: &str) -> SysResult<Option<RegData>> {
        self.enter("reg_read")?;
        Ok(self.state.borrow().reg.get(&rk(root, key, name)).cloned())
    }
    fn reg_write(&self, root: &RegRoot, key: &str, name: &str, data: &RegData) -> SysResult<()> {
        if self.enter("reg_write")? {
            self.state
                .borrow_mut()
                .add_reg(root, key, name, data.clone());
        }
        Ok(())
    }
    fn reg_delete(&self, root: &RegRoot, key: &str, name: &str) -> SysResult<()> {
        if self.enter("reg_delete")? {
            self.state.borrow_mut().reg.remove(&rk(root, key, name));
        }
        Ok(())
    }
    fn reg_key_exists(&self, root: &RegRoot, key: &str) -> SysResult<bool> {
        self.enter("reg_read")?;
        Ok(self
            .state
            .borrow()
            .reg_keys
            .contains(&(format!("{root:?}"), key.to_ascii_lowercase())))
    }
    fn reg_delete_key_if_empty(&self, root: &RegRoot, key: &str) -> SysResult<()> {
        if self.enter("reg_delete")? {
            let mut st = self.state.borrow_mut();
            let (r, k) = (format!("{root:?}"), key.to_ascii_lowercase());
            let has_values = st.reg.keys().any(|(rr, kk, _)| *rr == r && *kk == k);
            let prefix = format!("{k}\\");
            let has_children = st
                .reg_keys
                .iter()
                .any(|(rr, kk)| *rr == r && kk.starts_with(&prefix));
            if !has_values && !has_children {
                st.reg_keys.remove(&(r, k));
            }
        }
        Ok(())
    }
    fn service_start_type(&self, name: &str) -> SysResult<StartType> {
        self.enter("service_read")?;
        self.state
            .borrow()
            .services
            .get(name)
            .copied()
            .ok_or_else(|| SysError::NotFound(name.into()))
    }
    fn set_service_start_type(&self, name: &str, start: StartType) -> SysResult<()> {
        if self.enter("service_write")? {
            let mut st = self.state.borrow_mut();
            let s = st
                .services
                .get_mut(name)
                .ok_or_else(|| SysError::NotFound(name.into()))?;
            *s = start;
        }
        Ok(())
    }
    fn active_power_scheme(&self) -> SysResult<String> {
        self.enter("power_read")?;
        Ok(self.state.borrow().power.clone())
    }
    fn set_power_scheme(&self, guid: &str) -> SysResult<()> {
        if self.enter("power_write")? {
            self.state.borrow_mut().power = guid.to_ascii_lowercase();
        }
        Ok(())
    }
    fn power_schemes(&self) -> SysResult<Vec<String>> {
        self.enter("power_read")?;
        let st = self.state.borrow();
        let mut v: BTreeSet<String> = st.power_schemes.clone();
        v.insert(st.power.clone());
        Ok(v.into_iter().collect())
    }
    fn task_enabled(&self, path: &str) -> SysResult<bool> {
        self.enter("task_read")?;
        self.state
            .borrow()
            .tasks
            .get(path)
            .copied()
            .ok_or_else(|| SysError::NotFound(path.into()))
    }
    fn set_task_enabled(&self, path: &str, enabled: bool) -> SysResult<()> {
        if self.enter("task_write")? {
            let mut st = self.state.borrow_mut();
            let t = st
                .tasks
                .get_mut(path)
                .ok_or_else(|| SysError::NotFound(path.into()))?;
            *t = enabled;
        }
        Ok(())
    }
    fn proc_alive(&self, p: &ProcRef) -> SysResult<bool> {
        self.enter("proc_alive")?;
        let st = self.state.borrow();
        self.opened.borrow_mut().push(p.pid);
        Ok(st
            .procs
            .get(&p.pid)
            .is_some_and(|fp| fp.alive && fp.start_time == p.start_time))
    }
    fn proc_priority(&self, p: &ProcRef) -> SysResult<PriorityClass> {
        self.with_proc("proc_read", p, |fp, _| fp.priority)
    }
    fn set_proc_priority(&self, p: &ProcRef, c: PriorityClass) -> SysResult<()> {
        self.with_proc("proc_priority", p, |fp, m| {
            if m {
                fp.priority = c
            }
        })
    }
    fn proc_eco_qos(&self, p: &ProcRef) -> SysResult<Option<bool>> {
        self.with_proc("proc_read", p, |fp, _| Some(fp.eco_qos))
    }
    fn set_proc_eco_qos(&self, p: &ProcRef, on: bool) -> SysResult<()> {
        self.with_proc("proc_eco", p, |fp, m| {
            if m {
                fp.eco_qos = on
            }
        })
    }
    fn suspend(&self, p: &ProcRef) -> SysResult<()> {
        self.with_proc("proc_suspend", p, |fp, m| {
            if m {
                fp.suspended = true
            }
        })
    }
    fn resume(&self, p: &ProcRef) -> SysResult<()> {
        self.with_proc("proc_resume", p, |fp, m| {
            if m {
                fp.suspended = false
            }
        })
    }
    fn file_read(&self, path: &str) -> SysResult<Option<FileData>> {
        self.enter("file_read")?;
        Ok(self
            .state
            .borrow()
            .files
            .get(&path.to_ascii_lowercase())
            .map(|(b, ro)| FileData { bytes: b.clone(), readonly: *ro }))
    }
    fn file_write_atomic(&self, path: &str, bytes: &[u8]) -> SysResult<()> {
        // Atomic like the real one: either the whole new content or nothing.
        if self.enter("file_write")? {
            let mut st = self.state.borrow_mut();
            let k = path.to_ascii_lowercase();
            if st.files.get(&k).is_some_and(|(_, ro)| *ro) {
                return Err(SysError::AccessDenied(format!("{path} is read-only")));
            }
            st.files.insert(k, (bytes.to_vec(), false));
        }
        Ok(())
    }
    fn file_delete(&self, path: &str) -> SysResult<()> {
        if self.enter("file_delete")? {
            self.state.borrow_mut().files.remove(&path.to_ascii_lowercase());
        }
        Ok(())
    }
    fn driver_profiles(&self) -> bool {
        self.state.borrow().driver_profiles
    }
    fn driver_get(&self, exe: &str, setting: DriverSetting) -> SysResult<Option<u32>> {
        self.enter("driver_read")?;
        let st = self.state.borrow();
        if !st.driver_profiles {
            return Err(SysError::NotFound("driver profiles".into()));
        }
        Ok(st.driver.get(&(exe.to_ascii_lowercase(), setting)).copied())
    }
    fn driver_set(&self, exe: &str, setting: DriverSetting, value: u32) -> SysResult<()> {
        if self.enter("driver_write")? {
            self.state
                .borrow_mut()
                .driver
                .insert((exe.to_ascii_lowercase(), setting), value);
        }
        Ok(())
    }
    fn driver_clear(&self, exe: &str, setting: DriverSetting) -> SysResult<()> {
        if self.enter("driver_write")? {
            self.state
                .borrow_mut()
                .driver
                .remove(&(exe.to_ascii_lowercase(), setting));
        }
        Ok(())
    }
}
