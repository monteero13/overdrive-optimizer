//! Sharing methods (goals.md §7). The service never talks to the network: it only builds
//! payloads the UI shows verbatim, and the *user* sends them (GitHub issue in the browser,
//! a code pasted into a chat). Community presets come in the other way: the UI downloads
//! the public file only when the user opted in, and hands it over with `presets.import`.

use std::path::Path;

use serde_json::{json, Value};

use overdrive_core::share::{
    self, hw_class, Card, CardInput, CardMetrics, HelpGame, HelpHw,
    HelpRequest, HelpResponse, HelpTweak, Identity, Measurement, PresetFile, PresetStatus,
    ShareError, Signed, CARD_PREFIX, HELP_REQUEST_PREFIX, HELP_RESPONSE_PREFIX, SCHEMA,
};

use super::{bad, Core, Fail};
use crate::proto::{CommunityView, Metrics};

/// Public repository that hosts community measurements (issues) and presets.
pub const COMMUNITY_REPO: &str = "monteero13/overdrive-optimizer";

/// Load the per-install signing key, creating it on first use. It lives next to the journal
/// (ProgramData, writable only by the service).
pub fn load_identity(dir: &Path) -> Identity {
    let path = dir.join("identity.key");
    if let Ok(b) = std::fs::read(&path) {
        if let Ok(seed) = <[u8; 32]>::try_from(b.as_slice()) {
            return Identity::from_seed(seed);
        }
    }
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).expect("OS random generator");
    let _ = std::fs::write(&path, seed);
    Identity::from_seed(seed)
}

/// `YYYY-MM-DD` (UTC) from Unix milliseconds — days only, never the time of day.
pub fn day(ms: i64) -> String {
    let z = ms.div_euclid(86_400_000) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn share_fail(e: ShareError) -> Fail {
    let key = match e {
        ShareError::BadCode => "err.share_bad_code",
        ShareError::BadSignature => "err.share_tampered",
        ShareError::NewerSchema(_) => "err.share_newer",
        ShareError::NotMeasured => "err.share_not_measured",
    };
    bad("SHARE", key)
}

fn card_metrics(m: &Metrics) -> CardMetrics {
    CardMetrics { avg_fps: m.avg_fps, low1: m.low1, stdev_ms: m.stdev_ms }
}

impl Core {
    pub(super) fn sharing(&mut self, method: &str, p: &Value) -> Result<Value, Fail> {
        match method {
            "share.card" => {
                let tx = p.get("txId").and_then(Value::as_i64).ok_or_else(|| bad("BAD_PARAMS", "err.internal"))?;
                let r = self.reports.get(tx).ok_or_else(|| bad("NOT_FOUND", "err.not_found"))?;
                let hw = self.profile.as_ref().ok_or_else(|| bad("NO_PROFILE", "err.share_not_measured"))?;
                let name = self.game(&r.game_id).map(|g| g.name.clone()).unwrap_or_else(|| r.game_id.clone());
                let card = share::make_card(CardInput {
                    app: &self.cfg.svc_version,
                    game_id: &r.game_id,
                    game: &name,
                    hw: hw_class(hw),
                    bottleneck: &r.bottleneck,
                    baseline_runs: r.baseline_runs,
                    min_runs: self.settings.min_runs,
                    baseline: card_metrics(&r.baseline),
                    optimized: card_metrics(&r.optimized),
                    baseline_series: &r.baseline_series,
                    optimized_series: &r.optimized_series,
                    kept: r.tweaks.iter().filter(|t| t.verdict == "kept").map(|t| t.tweak_id.clone()).collect(),
                    day: &day(r.ended_at),
                })
                .map_err(share_fail)?;
                let signed = self.identity.sign(&card);
                Ok(json!({
                    "card": card,
                    "code": share::encode(CARD_PREFIX, &signed),
                    "shortId": signed.short_id(),
                    "low1DeltaPct": card.low1_delta_pct(),
                }))
            }
            "share.verifyCard" => {
                let code = Self::str_param(p, "code", 64 * 1024)?;
                match share::open::<Card>(CARD_PREFIX, code) {
                    Ok(s) => Ok(json!({ "valid": true, "card": s.payload, "shortId": s.short_id() })),
                    Err(e) => Ok(json!({ "valid": false, "error": share_fail(e).message_key })),
                }
            }
            "share.measurements" => {
                let items = self.measurements();
                if items.is_empty() {
                    return Err(bad("SHARE", "err.share_not_measured"));
                }
                let payloads: Vec<&Measurement> = items.iter().map(|s| &s.payload).collect();
                Ok(json!({
                    "count": items.len(),
                    // Exactly what the issue will contain, for the "what we send" preview.
                    "preview": payloads,
                    "pubkey": self.identity.public_hex(),
                    "issueUrl": share::issue_url(COMMUNITY_REPO, &items),
                }))
            }
            "presets.import" => {
                let file: PresetFile = serde_json::from_value(p.get("file").cloned().unwrap_or(Value::Null))
                    .map_err(|_| bad("BAD_PARAMS", "err.share_bad_code"))?;
                if file.schema > SCHEMA {
                    return Err(bad("SHARE", "err.share_newer"));
                }
                let n = file.presets.len();
                self.store.presets = Some(file);
                self.save();
                Ok(json!({ "presets": n }))
            }
            "presets.clear" => {
                self.store.presets = None;
                self.save();
                Ok(json!({ "ok": true }))
            }
            "help.request" => {
                let req = self.help_request()?;
                let signed = self.identity.sign(&req);
                let id = signed.short_id();
                if !self.store.help_requests.contains(&id) {
                    self.store.help_requests.push(id.clone());
                    let n = self.store.help_requests.len();
                    self.store.help_requests.drain(..n.saturating_sub(10));
                    self.save();
                }
                Ok(json!({ "code": share::encode(HELP_REQUEST_PREFIX, &signed), "shortId": id, "preview": req }))
            }
            "help.open" => {
                let code = Self::str_param(p, "code", 64 * 1024)?;
                let s = share::open::<HelpRequest>(HELP_REQUEST_PREFIX, code).map_err(share_fail)?;
                Ok(json!({ "request": s.payload, "shortId": s.short_id() }))
            }
            "help.respond" => {
                let code = Self::str_param(p, "requestCode", 64 * 1024)?;
                let req = share::open::<HelpRequest>(HELP_REQUEST_PREFIX, code).map_err(share_fail)?;
                let mut resp: HelpResponse = serde_json::from_value(json!({
                    "schema": SCHEMA,
                    "app": self.cfg.svc_version,
                    "request": req.short_id(),
                    "games": p.get("games").cloned().unwrap_or(json!([])),
                    "note": p.get("note").and_then(Value::as_str).unwrap_or(""),
                }))
                .map_err(|_| bad("BAD_PARAMS", "err.internal"))?;
                resp = resp.restricted_to(&req.payload);
                let signed = self.identity.sign(&resp);
                Ok(json!({ "code": share::encode(HELP_RESPONSE_PREFIX, &signed), "response": resp }))
            }
            "help.preview" | "help.apply" => {
                let code = Self::str_param(p, "code", 64 * 1024)?;
                let s = share::open::<HelpResponse>(HELP_RESPONSE_PREFIX, code).map_err(share_fail)?;
                if !self.store.help_requests.contains(&s.payload.request) {
                    return Err(bad("SHARE", "err.help_unknown_request"));
                }
                // Re-check against what this PC offers *now*: locks may have changed.
                let resp = s.payload.restricted_to(&self.help_request()?);
                if method == "help.apply" {
                    for g in &resp.games {
                        let t = self.store.toggles.entry(g.game_id.clone()).or_default();
                        for id in &g.enable {
                            t.insert(id.clone(), true);
                        }
                        for id in &g.disable {
                            t.insert(id.clone(), false);
                        }
                        self.armed.remove(&g.game_id);
                    }
                    self.save();
                }
                let changes: usize = resp.games.iter().map(|g| g.enable.len() + g.disable.len()).sum();
                Ok(json!({ "response": resp, "changes": changes, "applied": method == "help.apply" }))
            }
            _ => Err(bad("UNKNOWN_METHOD", "err.internal")),
        }
    }

    /// Every final A/B verdict on this PC, signed — nothing inconclusive, nothing unmeasured.
    fn measurements(&self) -> Vec<Signed<Measurement>> {
        let Some(hw) = &self.profile else { return vec![] };
        let class = hw_class(hw);
        let mut out = Vec::new();
        for g in &self.library {
            for o in self.journal.outcomes(&g.id).unwrap_or_default() {
                let (Some(d), Some(lo), Some(hi)) = (o.delta_pct, o.ci_low, o.ci_high) else { continue };
                if !share::shareable_verdict(&o.verdict) {
                    continue;
                }
                out.push(self.identity.sign(&Measurement {
                    schema: SCHEMA,
                    app: self.cfg.svc_version.clone(),
                    game_id: o.game_id,
                    hw: class.clone(),
                    tweak_id: o.tweak_id,
                    verdict: o.verdict,
                    delta_pct: d,
                    ci_low: lo,
                    ci_high: hi,
                    runs: o.runs,
                }));
            }
        }
        out
    }

    /// Redacted diagnostic for a friend: hardware models, structural issue ids and, per
    /// game, the plan (tweak ids, on/off, measured history, locks). No paths, no user
    /// names, no process list, no exe names.
    fn help_request(&self) -> Result<HelpRequest, Fail> {
        let hw = self.profile.as_ref().ok_or_else(|| bad("NO_PROFILE", "err.share_not_measured"))?;
        let mut games = Vec::new();
        for g in &self.library {
            let Ok(plan) = self.plan_preview(&g.id) else { continue };
            let target = self.target(g);
            games.push(HelpGame {
                id: g.id.clone(),
                name: g.name.clone(),
                anticheat: match target.anticheat {
                    overdrive_core::safety::AntiCheat::None => "none",
                    overdrive_core::safety::AntiCheat::UserMode(_) => "user_mode",
                    _ => "kernel",
                }
                .into(),
                baseline_runs: self.baseline_runs(&g.id),
                tweaks: plan
                    .items
                    .iter()
                    .map(|i| HelpTweak {
                        id: i.tweak_id.clone(),
                        enabled: i.enabled,
                        history: i.history.into(),
                        lock: i.lock.map(String::from),
                    })
                    .collect(),
            });
        }
        Ok(HelpRequest {
            schema: SCHEMA,
            app: self.cfg.svc_version.clone(),
            hw: HelpHw {
                tier: share::tier_num(hw.tier),
                laptop: hw.is_laptop(),
                cpu: hw.cpu.name.value.clone().unwrap_or_default(),
                gpus: hw.gpus.iter().map(|g| g.name.clone()).collect(),
                ram_gb: hw.memory.total_gb.val().map(|g| g.round() as u32).unwrap_or(0),
                ram_mts: hw.memory.configured_mts.val(),
                ram_rated_mts: hw.memory.rated_mts.val(),
                windows_build: hw.windows.build.val(),
            },
            issues: self.issues.iter().map(|i| i.id.clone()).collect(),
            games,
        })
    }

    /// Community result for a tweak in a game: same-hardware scope first, then tier.
    pub(super) fn community(&self, game_id: &str, tweak_id: &str) -> Option<CommunityView> {
        let file = self.store.presets.as_ref()?;
        let hw = hw_class(self.profile.as_ref()?);
        let tier = format!("tier:{}:{}:{}", hw.tier, if hw.laptop { "laptop" } else { "desktop" }, hw.gpu_vendor);
        let class = format!("class:{}", hw.class);
        [class, tier].iter().find_map(|scope| {
            file.presets
                .iter()
                .find(|p| p.game_id == game_id && p.tweak_id == tweak_id && &p.scope == scope)
                .map(|p| CommunityView {
                    status: match p.status {
                        PresetStatus::Verified => "verified",
                        PresetStatus::Harmful => "harmful",
                        PresetStatus::Pending => "pending",
                    },
                    installs: p.installs,
                    same_hardware: scope.starts_with("class:"),
                    // A gain is only ever shown once enough installs agree.
                    median_delta_pct: (p.status != PresetStatus::Pending).then_some(p.median_delta_pct),
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use overdrive_core::experiment::Verdict;

    #[test]
    fn day_is_utc_date_only() {
        assert_eq!(day(0), "1970-01-01");
        assert_eq!(day(1_763_251_200_000), "2025-11-16");
        assert_eq!(day(1_709_164_800_000), "2024-02-29");
    }

    #[test]
    fn verdict_names_are_the_shareable_ones() {
        for v in [Verdict::Kept, Verdict::NoBenefit, Verdict::Harmful] {
            assert!(share::shareable_verdict(v.as_str()));
        }
        assert!(!share::shareable_verdict(Verdict::Inconclusive.as_str()));
    }
}
