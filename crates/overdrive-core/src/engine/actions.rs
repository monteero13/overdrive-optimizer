//! capture / apply / verify / revert for every [`Action`]. All idempotent.
//!
//! Snapshots (`before`) are JSON so the journal can store them and a *different process*
//! (the service after a crash) can revert using only what's on disk.

use serde_json::{json, Value};

use super::{Action, RegData, RegTarget, SysError, SysResult, SystemOps};

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
    }
}

/// Does the system now match the intended state?
pub fn verify_applied(a: &Action, sys: &dyn SystemOps) -> SysResult<bool> {
    match a {
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
        _ => Ok(value_part(&capture(a, sys)?) != intended(a)),
    }
}
