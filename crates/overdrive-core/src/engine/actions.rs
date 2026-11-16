//! capture / apply / verify / revert for every [`Action`]. All idempotent.
//!
//! Snapshots (`before`) are JSON so the journal can store them and a *different process*
//! (the service after a crash) can revert using only what's on disk.

use base64::Engine as _;
use serde_json::{json, Value};

use super::{Action, DriverTarget, RegData, RegTarget, SysError, SysResult, SystemOps, MAX_PATCH_FILE};
use crate::config::{self, ConfigEdit, ConfigFormat};

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn cfg_err(e: config::ConfigError) -> SysError {
    SysError::Other(format!("config file: {e}"))
}

/// Original bytes of a file snapshot (`None` = the file did not exist).
fn file_from_snapshot(v: &Value) -> SysResult<Option<Vec<u8>>> {
    if v.get("absent").and_then(Value::as_bool) == Some(true) {
        return Ok(None);
    }
    let s = v
        .get("b64")
        .and_then(Value::as_str)
        .ok_or_else(|| SysError::Other(format!("bad file snapshot: {v}")))?;
    b64()
        .decode(s)
        .map(Some)
        .map_err(|e| SysError::Other(format!("bad file snapshot: {e}")))
}

fn current_bytes(sys: &dyn SystemOps, path: &str) -> SysResult<Option<Vec<u8>>> {
    Ok(sys.file_read(path)?.map(|f| f.bytes))
}

/// No key holds the value we set, unless it already had that value originally.
fn no_trace_left(cur: &[u8], orig: &[u8], format: ConfigFormat, edits: &[ConfigEdit]) -> SysResult<bool> {
    let now = config::read(cur, format, edits).map_err(cfg_err)?;
    let was = config::read(orig, format, edits).map_err(cfg_err)?;
    Ok(edits.iter().zip(now.iter().zip(&was)).all(|(e, (n, w))| {
        let ours = Some(e.value.trim());
        n.as_deref() != ours || w.as_deref() == ours
    }))
}

fn revert_file(sys: &dyn SystemOps, path: &str, format: ConfigFormat, edits: &[ConfigEdit], before: &Value) -> SysResult<()> {
    // We never create config files, so an "absent" snapshot means nothing was written.
    let Some(orig) = file_from_snapshot(before)? else {
        return Ok(());
    };
    let Some(cur) = current_bytes(sys, path)? else {
        return sys.file_write_atomic(path, &orig);
    };
    if cur == orig {
        return Ok(());
    }
    // Still exactly what we wrote → restore the original, byte for byte.
    if cur == config::patch(&orig, format, edits).map_err(cfg_err)? {
        return sys.file_write_atomic(path, &orig);
    }
    // The game rewrote the file (other settings may have changed in-game): undo only our
    // keys that still hold our value, keep everything else the user did.
    let now = config::read(&cur, format, edits).map_err(cfg_err)?;
    let was = config::read(&orig, format, edits).map_err(cfg_err)?;
    let (mut restore, mut drop) = (Vec::new(), Vec::new());
    for (e, (n, w)) in edits.iter().zip(now.iter().zip(&was)) {
        if n.as_deref() != Some(e.value.trim()) {
            continue; // changed by the user/game after us: theirs now
        }
        match w {
            Some(v) => restore.push(ConfigEdit { value: v.clone(), ..e.clone() }),
            None => drop.push(e.clone()),
        }
    }
    let mut out = config::patch(&cur, format, &restore).map_err(cfg_err)?;
    if !drop.is_empty() {
        out = config::remove(&out, format, &drop).map_err(cfg_err)?;
    }
    if out != cur {
        sys.file_write_atomic(path, &out)?;
    }
    Ok(())
}

fn reg_snapshot(v: Option<RegData>) -> Value {
    match v {
        None => json!({ "absent": true }),
        Some(d) => json!({ "value": d }),
    }
}

fn segments(key: &str) -> Vec<&str> {
    key.split('\\').filter(|s| !s.is_empty()).collect()
}

/// First ancestor of `key` (or the key itself) that doesn't exist, so revert can remove
/// exactly the empty keys that applying created — and nothing else.
fn missing_from(
    sys: &dyn SystemOps,
    root: &super::RegRoot,
    key: &str,
) -> SysResult<Option<String>> {
    let segs = segments(key);
    for i in 1..=segs.len() {
        let path = segs[..i].join("\\");
        if !sys.reg_key_exists(root, &path)? {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// Registry snapshot without key-existence bookkeeping (what "same value" means).
fn value_part(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("missingFrom");
    }
    v
}

fn reg_from_snapshot(v: &Value) -> SysResult<Option<RegData>> {
    if v.get("absent").and_then(Value::as_bool) == Some(true) {
        return Ok(None);
    }
    let d = v
        .get("value")
        .cloned()
        .ok_or_else(|| SysError::Other(format!("bad registry snapshot: {v}")))?;
    serde_json::from_value(d)
        .map(Some)
        .map_err(|e| SysError::Other(e.to_string()))
}

fn field<T: serde::de::DeserializeOwned>(v: &Value, k: &str) -> SysResult<T> {
    serde_json::from_value(v.get(k).cloned().unwrap_or(Value::Null))
        .map_err(|e| SysError::Other(format!("snapshot field {k}: {e}")))
}

/// State the action wants to produce, as a snapshot (used to detect third-party changes).
pub fn intended(a: &Action) -> Value {
    match a {
        Action::RegistryValue { target, .. } => match target {
            RegTarget::Set(d) => reg_snapshot(Some(d.clone())),
            RegTarget::Delete => reg_snapshot(None),
        },
        Action::ServiceStart { start, .. } => json!({ "start": start }),
        Action::PowerScheme { guid } => json!({ "guid": guid.to_ascii_lowercase() }),
        Action::ProcessPriority { class, .. } => json!({ "class": class }),
        Action::ProcessEcoQos { on, .. } => json!({ "on": on }),
        Action::ProcessSuspend { .. } => json!({ "suspended": true }),
        Action::ScheduledTask { enabled, .. } => json!({ "enabled": enabled }),
        Action::DriverSetting { target, .. } => match target {
            DriverTarget::Set(v) => json!({ "value": v }),
            DriverTarget::Clear => json!({ "absent": true }),
        },
        // The resulting bytes depend on the original: the journal keeps the edits instead.
        Action::FilePatch { edits, .. } => json!({ "edits": edits }),
    }
}

/// Current state in snapshot form. For a process that's gone: `{"gone": true}`.
pub fn capture(a: &Action, sys: &dyn SystemOps) -> SysResult<Value> {
    if let Some(p) = a.process() {
        if !sys.proc_alive(p)? {
            return Ok(json!({ "gone": true }));
        }
    }
    Ok(match a {
        Action::RegistryValue {
            root, key, name, ..
        } => {
            let mut s = reg_snapshot(sys.reg_read(root, key, name)?);
            s["missingFrom"] = json!(missing_from(sys, root, key)?);
            s
        }
        Action::ServiceStart { name, .. } => json!({ "start": sys.service_start_type(name)? }),
        Action::PowerScheme { .. } => {
            json!({ "guid": sys.active_power_scheme()?.to_ascii_lowercase() })
        }
        Action::ProcessPriority { process, .. } => json!({ "class": sys.proc_priority(process)? }),
        Action::ProcessEcoQos { process, .. } => json!({ "on": sys.proc_eco_qos(process)? }),
        // There is no reliable "is suspended" query: we only ever suspend processes the
        // policy saw running in the foreground session, so the original state is "running".
        Action::ProcessSuspend { .. } => json!({ "suspended": false }),
        Action::ScheduledTask { path, .. } => json!({ "enabled": sys.task_enabled(path)? }),
        Action::DriverSetting { exe, setting, .. } => match sys.driver_get(exe, *setting)? {
            Some(v) => json!({ "value": v }),
            None => json!({ "absent": true }),
        },
        Action::FilePatch { path, .. } => match sys.file_read(path)? {
            None => json!({ "absent": true }),
            Some(f) if f.bytes.len() > MAX_PATCH_FILE => {
                return Err(SysError::Other(format!("{path}: too big to be a config file")))
            }
            Some(f) => json!({ "b64": b64().encode(&f.bytes), "readonly": f.readonly }),
        },
    })
}

pub fn apply(a: &Action, sys: &dyn SystemOps) -> SysResult<()> {
    match a {
        Action::RegistryValue {
            root,
            key,
            name,
            target,
        } => match target {
            RegTarget::Set(d) => sys.reg_write(root, key, name, d),
            RegTarget::Delete => sys.reg_delete(root, key, name),
        },
        Action::ServiceStart { name, start } => sys.set_service_start_type(name, *start),
        Action::PowerScheme { guid } => sys.set_power_scheme(guid),
        Action::ProcessPriority { process, class } => sys.set_proc_priority(process, *class),
        Action::ProcessEcoQos { process, on } => sys.set_proc_eco_qos(process, *on),
        Action::ProcessSuspend { process } => sys.suspend(process),
        Action::ScheduledTask { path, enabled } => sys.set_task_enabled(path, *enabled),
        Action::DriverSetting { exe, setting, target } => match target {
            DriverTarget::Set(v) => sys.driver_set(exe, *setting, *v),
            DriverTarget::Clear => sys.driver_clear(exe, *setting),
        },
        Action::FilePatch { path, format, edits } => {
            let f = sys
                .file_read(path)?
                .ok_or_else(|| SysError::NotFound(path.clone()))?;
            // The user locked this file on purpose: respect it, never clear the flag.
            if f.readonly {
                return Err(SysError::AccessDenied(format!("{path} is read-only")));
            }
            let out = config::patch(&f.bytes, *format, edits).map_err(cfg_err)?;
            if out == f.bytes {
                return Ok(());
            }
            sys.file_write_atomic(path, &out)
        }
    }
}

/// Does the system now match the intended state?
pub fn verify_applied(a: &Action, sys: &dyn SystemOps) -> SysResult<bool> {
    match a {
        Action::FilePatch { path, format, edits } => match current_bytes(sys, path)? {
            Some(b) => config::holds(&b, *format, edits).map_err(cfg_err),
            None => Ok(false),
        },
        Action::ProcessSuspend { process } => sys.proc_alive(process),
        Action::ProcessEcoQos { .. } => {
            let now = capture(a, sys)?;
            // Windows 10 can set EcoQoS but not report it: the successful SetProcessInformation
            // is the only evidence available. Documented as "unverifiable on this build".
            Ok(now.get("on").is_some_and(Value::is_null) || now == intended(a))
        }
        _ => Ok(value_part(&capture(a, sys)?) == intended(a)),
    }
}

/// Restore exactly `before`. A process that exited needs nothing.
pub fn revert(a: &Action, sys: &dyn SystemOps, before: &Value) -> SysResult<()> {
    if before.get("gone").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    if let Some(p) = a.process() {
        if !sys.proc_alive(p)? {
            return Ok(());
        }
    }
    match a {
        Action::RegistryValue {
            root, key, name, ..
        } => {
            match reg_from_snapshot(before)? {
                Some(d) => sys.reg_write(root, key, name, &d)?,
                None => sys.reg_delete(root, key, name)?,
            }
            // Remove the (now empty) keys that applying created, deepest first.
            if let Some(m) = before.get("missingFrom").and_then(Value::as_str) {
                let segs = segments(key);
                let stop = segments(m).len();
                for i in (stop..=segs.len()).rev() {
                    sys.reg_delete_key_if_empty(root, &segs[..i].join("\\"))?;
                }
            }
            Ok(())
        }
        Action::ServiceStart { name, .. } => {
            sys.set_service_start_type(name, field(before, "start")?)
        }
        Action::PowerScheme { .. } => sys.set_power_scheme(&field::<String>(before, "guid")?),
        Action::ProcessPriority { process, .. } => {
            sys.set_proc_priority(process, field(before, "class")?)
        }
        // Unknown original (not queryable) → back to the default, "let Windows decide".
        Action::ProcessEcoQos { process, .. } => sys.set_proc_eco_qos(
            process,
            field::<Option<bool>>(before, "on")?.unwrap_or(false),
        ),
        Action::ProcessSuspend { process } => sys.resume(process),
        Action::ScheduledTask { path, .. } => sys.set_task_enabled(path, field(before, "enabled")?),
        Action::DriverSetting { exe, setting, .. } => match before.get("value") {
            Some(_) => sys.driver_set(exe, *setting, field(before, "value")?),
            None => sys.driver_clear(exe, *setting),
        },
        Action::FilePatch { path, format, edits } => revert_file(sys, path, *format, edits, before),
    }
}

pub fn verify_reverted(a: &Action, sys: &dyn SystemOps, before: &Value) -> SysResult<bool> {
    if let Some(p) = a.process() {
        if !sys.proc_alive(p)? {
            return Ok(true); // nothing left to restore
        }
    }
    match a {
        Action::ProcessSuspend { process } => sys.proc_alive(process),
        Action::FilePatch { path, format, edits } => {
            let Some(orig) = file_from_snapshot(before)? else {
                return Ok(true);
            };
            match sys.file_read(path)? {
                None => Ok(false),
                Some(f) if f.bytes == orig => Ok(true),
                Some(f) => no_trace_left(&f.bytes, &orig, *format, edits),
            }
        }
        _ => Ok(&capture(a, sys)? == before),
    }
}

/// Before reverting: has someone else (user, Windows, another app) changed the target
/// since we applied it? If so we must NOT overwrite their choice.
pub fn changed_by_other(a: &Action, sys: &dyn SystemOps) -> SysResult<bool> {
    match a {
        // Process state is ours for the duration of the session.
        Action::ProcessPriority { .. }
        | Action::ProcessEcoQos { .. }
        | Action::ProcessSuspend { .. } => Ok(false),
        // Superseded only when *none* of our keys hold any more (the game reset them all);
        // partial cases are handled key by key in `revert_file`.
        Action::FilePatch { path, format, edits } => match current_bytes(sys, path)? {
            None => Ok(true),
            Some(b) => {
                let now = config::read(&b, *format, edits).map_err(cfg_err)?;
                Ok(edits.iter().zip(&now).all(|(e, n)| n.as_deref() != Some(e.value.trim())))
            }
        },
        _ => Ok(value_part(&capture(a, sys)?) != intended(a)),
    }
}
