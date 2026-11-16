//! Dry-run system: every read sees the real machine, every write lands in an in-memory
//! overlay. The engine, the journal and the A/B protocol behave exactly as in production,
//! but nothing on the PC changes and no process is ever touched (process state lives in
//! the overlay too). Used by `overdrive-svc console --dry-run`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use overdrive_core::engine::{
    PriorityClass, ProcRef, RegData, RegRoot, StartType, SysError, SysResult, SystemOps,
};

type RegKey = (RegRoot, String, String);

#[derive(Default)]
struct Layer {
    /// `None` = deleted in the overlay.
    reg: HashMap<RegKey, Option<RegData>>,
    created_keys: HashSet<(RegRoot, String)>,
    removed_keys: HashSet<(RegRoot, String)>,
    services: HashMap<String, StartType>,
    power: Option<String>,
    tasks: HashMap<String, bool>,
    priority: HashMap<(u32, u64), PriorityClass>,
    eco: HashMap<(u32, u64), bool>,
    suspended: HashSet<(u32, u64)>,
}

pub struct OverlaySystem<S: SystemOps> {
    real: S,
    layer: RefCell<Layer>,
}

fn k(root: &RegRoot, key: &str, name: &str) -> RegKey {
    (root.clone(), key.to_ascii_lowercase(), name.to_ascii_lowercase())
}

impl<S: SystemOps> OverlaySystem<S> {
    pub fn new(real: S) -> Self {
        Self {
            real,
            layer: RefCell::new(Layer::default()),
        }
    }

    /// How many simulated changes are currently "in effect" (diagnostics / tests).
    pub fn pending_changes(&self) -> usize {
        let l = self.layer.borrow();
        l.reg.len() + l.services.len() + l.tasks.len() + usize::from(l.power.is_some())
    }

    fn key_exists_real(&self, root: &RegRoot, key: &str) -> SysResult<bool> {
        self.real.reg_key_exists(root, key)
    }
}

impl<S: SystemOps> SystemOps for OverlaySystem<S> {
    fn reg_read(&self, root: &RegRoot, key: &str, name: &str) -> SysResult<Option<RegData>> {
        if let Some(v) = self.layer.borrow().reg.get(&k(root, key, name)) {
            return Ok(v.clone());
        }
        self.real.reg_read(root, key, name)
    }
    fn reg_write(&self, root: &RegRoot, key: &str, name: &str, data: &RegData) -> SysResult<()> {
        let exists = self.reg_key_exists(root, key)?;
        let mut l = self.layer.borrow_mut();
        if !exists {
            // Track the whole chain the real registry would have created.
            let segs: Vec<&str> = key.split('\\').filter(|s| !s.is_empty()).collect();
            for i in 1..=segs.len() {
                l.created_keys
                    .insert((root.clone(), segs[..i].join("\\").to_ascii_lowercase()));
            }
        }
        l.removed_keys.remove(&(root.clone(), key.to_ascii_lowercase()));
        l.reg.insert(k(root, key, name), Some(data.clone()));
        Ok(())
    }
    fn reg_delete(&self, root: &RegRoot, key: &str, name: &str) -> SysResult<()> {
        self.layer.borrow_mut().reg.insert(k(root, key, name), None);
        Ok(())
    }
    fn reg_key_exists(&self, root: &RegRoot, key: &str) -> SysResult<bool> {
        let id = (root.clone(), key.to_ascii_lowercase());
        {
            let l = self.layer.borrow();
            if l.removed_keys.contains(&id) {
                return Ok(false);
            }
            if l.created_keys.contains(&id) {
                return Ok(true);
            }
        }
        self.key_exists_real(root, key)
    }
    fn reg_delete_key_if_empty(&self, root: &RegRoot, key: &str) -> SysResult<()> {
        let id = (root.clone(), key.to_ascii_lowercase());
        let mut l = self.layer.borrow_mut();
        if !l.created_keys.contains(&id) {
            return Ok(()); // only keys the overlay created can disappear
        }
        let prefix = format!("{}\\", id.1);
        let has_values = l
            .reg
            .iter()
            .any(|((r, kk, _), v)| *r == id.0 && *kk == id.1 && v.is_some());
        let has_children = l
            .created_keys
            .iter()
            .any(|(r, kk)| *r == id.0 && kk.starts_with(&prefix));
        if !has_values && !has_children {
            l.created_keys.remove(&id);
            l.reg.retain(|(r, kk, _), _| !(*r == id.0 && *kk == id.1));
            l.removed_keys.insert(id);
        }
        Ok(())
    }
    fn service_start_type(&self, name: &str) -> SysResult<StartType> {
        if let Some(s) = self.layer.borrow().services.get(name) {
            return Ok(*s);
        }
        self.real.service_start_type(name)
    }
    fn set_service_start_type(&self, name: &str, start: StartType) -> SysResult<()> {
        self.real.service_start_type(name)?; // must exist
        self.layer.borrow_mut().services.insert(name.into(), start);
        Ok(())
    }
    fn active_power_scheme(&self) -> SysResult<String> {
        if let Some(p) = &self.layer.borrow().power {
            return Ok(p.clone());
        }
        self.real.active_power_scheme()
    }
    fn set_power_scheme(&self, guid: &str) -> SysResult<()> {
        let g = guid.to_ascii_lowercase();
        if !self.real.power_schemes()?.contains(&g) {
            return Err(SysError::NotFound(format!("power scheme {g}")));
        }
        self.layer.borrow_mut().power = Some(g);
        Ok(())
    }
    fn power_schemes(&self) -> SysResult<Vec<String>> {
        self.real.power_schemes()
    }
    fn task_enabled(&self, path: &str) -> SysResult<bool> {
        if let Some(t) = self.layer.borrow().tasks.get(path) {
            return Ok(*t);
        }
        self.real.task_enabled(path)
    }
    fn set_task_enabled(&self, path: &str, enabled: bool) -> SysResult<()> {
        self.real.task_enabled(path)?;
        self.layer.borrow_mut().tasks.insert(path.into(), enabled);
        Ok(())
    }
    fn proc_alive(&self, p: &ProcRef) -> SysResult<bool> {
        self.real.proc_alive(p)
    }
    fn proc_priority(&self, p: &ProcRef) -> SysResult<PriorityClass> {
        if let Some(c) = self.layer.borrow().priority.get(&(p.pid, p.start_time)) {
            return Ok(*c);
        }
        self.real.proc_priority(p)
    }
    fn set_proc_priority(&self, p: &ProcRef, c: PriorityClass) -> SysResult<()> {
        self.layer
            .borrow_mut()
            .priority
            .insert((p.pid, p.start_time), c);
        Ok(())
    }
    fn proc_eco_qos(&self, p: &ProcRef) -> SysResult<Option<bool>> {
        if let Some(e) = self.layer.borrow().eco.get(&(p.pid, p.start_time)) {
            return Ok(Some(*e));
        }
        Ok(Some(false))
    }
    fn set_proc_eco_qos(&self, p: &ProcRef, on: bool) -> SysResult<()> {
        self.layer.borrow_mut().eco.insert((p.pid, p.start_time), on);
        Ok(())
    }
    fn suspend(&self, p: &ProcRef) -> SysResult<()> {
        self.layer
            .borrow_mut()
            .suspended
            .insert((p.pid, p.start_time));
        Ok(())
    }
    fn resume(&self, p: &ProcRef) -> SysResult<()> {
        self.layer
            .borrow_mut()
            .suspended
            .remove(&(p.pid, p.start_time));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use overdrive_core::engine::fake::{FakeState, FakeSystem};

    #[test]
    fn writes_never_reach_the_real_system() {
        let mut st = FakeState {
            power: "balanced".into(),
            ..Default::default()
        };
        st.power_schemes.insert("high".into());
        st.add_reg(&RegRoot::CurrentUser, r"System\GameConfigStore", "GameDVR_Enabled", RegData::Dword(1));
        let real = FakeSystem::new(st.clone());
        let o = OverlaySystem::new(real);
        let hk = RegRoot::CurrentUser;
        o.reg_write(&hk, r"System\GameConfigStore", "GameDVR_Enabled", &RegData::Dword(0)).unwrap();
        o.reg_write(&hk, r"Software\New\Key", "v", &RegData::Dword(5)).unwrap();
        o.set_power_scheme("high").unwrap();
        assert_eq!(o.reg_read(&hk, r"System\GameConfigStore", "GameDVR_Enabled").unwrap(), Some(RegData::Dword(0)));
        assert!(o.reg_key_exists(&hk, r"Software\New").unwrap());
        assert_eq!(o.active_power_scheme().unwrap(), "high");
        assert_eq!(o.real.snapshot(), st, "real system untouched");
        // Undo in the overlay: delete value + empty key.
        o.reg_delete(&hk, r"Software\New\Key", "v").unwrap();
        o.reg_delete_key_if_empty(&hk, r"Software\New\Key").unwrap();
        o.reg_delete_key_if_empty(&hk, r"Software\New").unwrap();
        assert!(!o.reg_key_exists(&hk, r"Software\New").unwrap());
        assert!(o.set_power_scheme("nope").is_err(), "unknown schemes still fail like the real thing");
    }
}
