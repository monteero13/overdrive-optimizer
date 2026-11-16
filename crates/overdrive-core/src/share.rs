//! Sharing (goals.md §7): frametime cards, community measurements/presets and "help a
//! friend" codes. Pure: nothing here does I/O or networking.
//!
//! Privacy rules, enforced by construction and by tests:
//! - only *measured* data (valid sessions, A/B verdicts) ever becomes shareable;
//! - no user names, paths, process lists, serials or exe names — hardware is a coarse class
//!   (tier, laptop, GPU vendor) plus a hash of (CPU model, GPU model, RAM size);
//! - every payload is plain JSON the UI shows *verbatim* before anything leaves the PC.
//!
//! Integrity: payloads are signed with a per-install ed25519 key. A signature proves the
//! payload wasn't edited after it was made by *some* Overdrive install; it cannot prove the
//! install is honest (the code is open). Community presets therefore also require several
//! independent installs to agree.

use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::profile::{GpuVendor, HardwareProfile};
use crate::profile::Tier;

pub const SCHEMA: u32 = 1;
/// Largest code we accept (pasted text) and largest inflated payload (zip-bomb guard).
const MAX_CODE: usize = 64 * 1024;
const MAX_JSON: usize = 256 * 1024;
const CARD_POINTS: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShareError {
    #[error("not an Overdrive code")]
    BadCode,
    #[error("the code was modified after it was made")]
    BadSignature,
    #[error("made by a newer Overdrive (schema {0})")]
    NewerSchema(u32),
    #[error("not enough measured data to share")]
    NotMeasured,
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

pub fn sha256_hex(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

// ------------------------------------------------------------------ identity

/// Per-install signing key. The public key is a pseudonymous id: it links an install's own
/// submissions (needed to count *independent* measurements) and nothing else.
pub struct Identity {
    key: SigningKey,
}

impl Identity {
    /// From 32 random bytes (the service keeps them in its protected data folder).
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self { key: SigningKey::from_bytes(&seed) }
    }
    pub fn public_hex(&self) -> String {
        hex(self.key.verifying_key().as_bytes())
    }
    pub fn sign<T: Serialize + Clone>(&self, payload: &T) -> Signed<T> {
        let bytes = serde_json::to_vec(payload).expect("payload serialises");
        Signed {
            payload: payload.clone(),
            pubkey: self.public_hex(),
            sig: hex(&self.key.sign(&bytes).to_bytes()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signed<T> {
    pub payload: T,
    pub pubkey: String,
    pub sig: String,
}

impl<T: Serialize> Signed<T> {
    pub fn verify(&self) -> Result<(), ShareError> {
        let pk: [u8; 32] = unhex(&self.pubkey).and_then(|v| v.try_into().ok()).ok_or(ShareError::BadSignature)?;
        let sig: [u8; 64] = unhex(&self.sig).and_then(|v| v.try_into().ok()).ok_or(ShareError::BadSignature)?;
        let vk = VerifyingKey::from_bytes(&pk).map_err(|_| ShareError::BadSignature)?;
        let bytes = serde_json::to_vec(&self.payload).map_err(|_| ShareError::BadSignature)?;
        vk.verify(&bytes, &Signature::from_bytes(&sig)).map_err(|_| ShareError::BadSignature)
    }

    /// Short id printed on cards ("verify with code …").
    pub fn short_id(&self) -> String {
        sha256_hex(self.sig.as_bytes())[..8].to_string()
    }
}

/// `<prefix>.<base64url(deflate(json))>` — short enough to paste into a chat.
pub fn encode<T: Serialize>(prefix: &str, v: &T) -> String {
    let json = serde_json::to_vec(v).expect("serialises");
    let z = miniz_oxide::deflate::compress_to_vec(&json, 9);
    format!("{prefix}.{}", b64().encode(z))
}

pub fn decode<T: DeserializeOwned>(prefix: &str, code: &str) -> Result<T, ShareError> {
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if code.len() > MAX_CODE {
        return Err(ShareError::BadCode);
    }
    let body = code.strip_prefix(prefix).and_then(|r| r.strip_prefix('.')).ok_or(ShareError::BadCode)?;
    let z = b64().decode(body).map_err(|_| ShareError::BadCode)?;
    let json = miniz_oxide::inflate::decompress_to_vec_with_limit(&z, MAX_JSON).map_err(|_| ShareError::BadCode)?;
    serde_json::from_slice(&json).map_err(|_| ShareError::BadCode)
}

/// Decode + verify signature + schema check, for any signed payload with a `schema` field.
pub fn open<T: DeserializeOwned + Serialize + Versioned>(prefix: &str, code: &str) -> Result<Signed<T>, ShareError> {
    let s: Signed<T> = decode(prefix, code)?;
    s.verify()?;
    if s.payload.schema() > SCHEMA {
        return Err(ShareError::NewerSchema(s.payload.schema()));
    }
    Ok(s)
}

pub trait Versioned {
    fn schema(&self) -> u32;
}

// ------------------------------------------------------------------ hardware class

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HwClass {
    pub tier: u8,
    pub laptop: bool,
    /// nvidia | amd | intel | other — never the model.
    pub gpu_vendor: String,
    /// sha256("cpu|gpu|ram_gb|laptop")[..16]: same hardware ⇒ same class, model names not sent.
    pub class: String,
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase()
}

pub fn tier_num(t: Tier) -> u8 {
    match t {
        Tier::Tier1 => 1,
        Tier::Tier2 => 2,
        Tier::Tier3 => 3,
    }
}

pub fn hw_class(p: &HardwareProfile) -> HwClass {
    let gpu = p.dedicated_gpu().or(p.gpus.first());
    let vendor = match gpu.map(|g| g.vendor) {
        Some(GpuVendor::Nvidia) => "nvidia",
        Some(GpuVendor::Amd) => "amd",
        Some(GpuVendor::Intel) => "intel",
        _ => "other",
    };
    let ram = p.memory.total_gb.val().map(|g| g.round() as u32).unwrap_or(0);
    let key = format!(
        "{}|{}|{ram}|{}",
        norm(p.cpu.name.value.as_deref().unwrap_or("")),
        norm(gpu.map(|g| g.name.as_str()).unwrap_or("")),
        p.is_laptop()
    );
    HwClass {
        tier: tier_num(p.tier),
        laptop: p.is_laptop(),
        gpu_vendor: vendor.into(),
        class: sha256_hex(key.as_bytes())[..16].to_string(),
    }
}

// ------------------------------------------------------------------ frametime card

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CardMetrics {
    pub avg_fps: f64,
    pub low1: f64,
    pub stdev_ms: f64,
}

/// A before/after card. Built only from a measured session report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Card {
    pub schema: u32,
    pub app: String,
    /// Public library id (`steam:730`) and display name.
    pub game_id: String,
    pub game: String,
    pub hw: HwClass,
    pub bottleneck: String,
    pub baseline_runs: usize,
    pub baseline: CardMetrics,
    pub optimized: CardMetrics,
    /// Frametimes (ms), downsampled to at most 120 points, 0.1 ms precision.
    pub baseline_series: Vec<f64>,
    pub optimized_series: Vec<f64>,
    /// Tweaks the A/B protocol kept (ids).
    pub kept: Vec<String>,
    /// Day only (YYYY-MM-DD): no time of day.
    pub day: String,
}

impl Versioned for Card {
    fn schema(&self) -> u32 {
        self.schema
    }
}

impl Card {
    pub fn low1_delta_pct(&self) -> f64 {
        if self.baseline.low1 <= 0.0 {
            return 0.0;
        }
        ((self.optimized.low1 - self.baseline.low1) / self.baseline.low1 * 1000.0).round() / 10.0
    }
}

/// What a card is made from (the service fills it from a stored session report).
pub struct CardInput<'a> {
    pub app: &'a str,
    pub game_id: &'a str,
    pub game: &'a str,
    pub hw: HwClass,
    pub bottleneck: &'a str,
    pub baseline_runs: usize,
    pub min_runs: usize,
    pub baseline: CardMetrics,
    pub optimized: CardMetrics,
    pub baseline_series: &'a [f64],
    pub optimized_series: &'a [f64],
    pub kept: Vec<String>,
    pub day: &'a str,
}

fn downsample(v: &[f64]) -> Vec<f64> {
    if v.is_empty() {
        return vec![];
    }
    let n = v.len().min(CARD_POINTS);
    (0..n)
        .map(|i| {
            let (a, b) = (i * v.len() / n, ((i + 1) * v.len() / n).max(i * v.len() / n + 1));
            // Max of each bucket: a downsampled graph must never hide a stutter.
            let m = v[a..b].iter().copied().fold(f64::MIN, f64::max);
            (m * 10.0).round() / 10.0
        })
        .collect()
}

pub fn make_card(i: CardInput) -> Result<Card, ShareError> {
    let measured = i.baseline_runs >= i.min_runs
        && !i.baseline_series.is_empty()
        && !i.optimized_series.is_empty()
        && i.baseline.low1 > 0.0
        && i.optimized.low1 > 0.0;
    if !measured {
        return Err(ShareError::NotMeasured);
    }
    let r = |m: CardMetrics| CardMetrics {
        avg_fps: (m.avg_fps * 10.0).round() / 10.0,
        low1: (m.low1 * 10.0).round() / 10.0,
        stdev_ms: (m.stdev_ms * 100.0).round() / 100.0,
    };
    let mut kept = i.kept;
    kept.sort();
    Ok(Card {
        schema: SCHEMA,
        app: i.app.into(),
        game_id: i.game_id.into(),
        game: i.game.into(),
        hw: i.hw,
        bottleneck: i.bottleneck.into(),
        baseline_runs: i.baseline_runs,
        baseline: r(i.baseline),
        optimized: r(i.optimized),
        baseline_series: downsample(i.baseline_series),
        optimized_series: downsample(i.optimized_series),
        kept,
        day: i.day.chars().take(10).collect(),
    })
}

pub const CARD_PREFIX: &str = "ODC1";

// ------------------------------------------------------------------ community measurements

/// One A/B verdict of one tweak for one game on one install — the unit of community presets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Measurement {
    pub schema: u32,
    pub app: String,
    pub game_id: String,
    pub hw: HwClass,
    pub tweak_id: String,
    /// kept | no_benefit | harmful
    pub verdict: String,
    pub delta_pct: f64,
    pub ci_low: f64,
    pub ci_high: f64,
    pub runs: i64,
}

impl Versioned for Measurement {
    fn schema(&self) -> u32 {
        self.schema
    }
}

pub const MEASUREMENT_PREFIX: &str = "ODM1";
/// Issue title marker the aggregation workflow looks for.
pub const ISSUE_MARKER: &str = "[overdrive-measurement]";

/// Only final verdicts are shareable: an inconclusive test says nothing yet.
pub fn shareable_verdict(v: &str) -> bool {
    matches!(v, "kept" | "no_benefit" | "harmful")
}

/// The GitHub "new issue" URL the UI opens in the browser (the user presses Submit there).
pub fn issue_url(repo: &str, signed: &[Signed<Measurement>]) -> String {
    let code = encode(MEASUREMENT_PREFIX, &signed.to_vec());
    let body = format!(
        "Mediciones A/B de Overdrive (anónimas). / Overdrive A/B measurements (anonymous).\n\n```\n{code}\n```\n\n<details><summary>JSON</summary>\n\n```json\n{}\n```\n</details>\n",
        serde_json::to_string_pretty(signed).expect("serialises")
    );
    let title = format!("{ISSUE_MARKER} {} × {}", signed.first().map(|m| m.payload.game_id.as_str()).unwrap_or(""), signed.len());
    format!("https://github.com/{repo}/issues/new?title={}&body={}", url_encode(&title), url_encode(&body))
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Extract every measurement code from an issue body (used by the aggregation workflow).
/// Every measurement in a set of issue bodies → presets (the GitHub workflow runs this
/// through `cargo run -p overdrive-core --example presets`).
pub fn aggregate_issue_bodies(bodies: &[String], day: &str) -> PresetFile {
    let items: Vec<Signed<Measurement>> = bodies
        .iter()
        .flat_map(|b| codes_in(b))
        .filter_map(|c| decode::<Vec<Signed<Measurement>>>(MEASUREMENT_PREFIX, &c).ok())
        .flatten()
        .collect();
    aggregate(&items, &AggregateRules::default(), day)
}

pub fn codes_in(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || c == '`')
        .filter(|t| t.starts_with(&format!("{MEASUREMENT_PREFIX}.")))
        .map(String::from)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetStatus {
    /// Enough independent installs agree it helps.
    Verified,
    /// Enough independent installs agree it hurts.
    Harmful,
    /// Not enough agreement yet: shown as "being measured", never as a gain.
    Pending,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preset {
    pub game_id: String,
    pub tweak_id: String,
    /// `class:<hash>` (same hardware) or `tier:<n>:<laptop|desktop>:<vendor>`.
    pub scope: String,
    pub installs: usize,
    pub kept: usize,
    pub harmful: usize,
    /// Median Δ of the 1 % low (or the tweak's own metric) across installs, %.
    pub median_delta_pct: f64,
    pub status: PresetStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresetFile {
    pub schema: u32,
    pub generated_day: String,
    pub min_installs: usize,
    pub presets: Vec<Preset>,
}

pub struct AggregateRules {
    /// Independent installs needed before anything is claimed.
    pub min_installs: usize,
    /// Share of those installs whose verdict must agree.
    pub agreement: f64,
    pub min_gain_pct: f64,
}

impl Default for AggregateRules {
    fn default() -> Self {
        Self { min_installs: 5, agreement: 0.8, min_gain_pct: 3.0 }
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    let m = if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 };
    (m * 10.0).round() / 10.0
}

/// Turn submitted measurements into presets. Invalid signatures, unknown schemas and
/// inconclusive verdicts are dropped; each install counts once per (game, hardware, tweak)
/// — its latest submission wins.
pub fn aggregate(items: &[Signed<Measurement>], rules: &AggregateRules, day: &str) -> PresetFile {
    use std::collections::BTreeMap;
    let mut latest: BTreeMap<(String, String, String, String), &Measurement> = BTreeMap::new();
    for s in items {
        let m = &s.payload;
        if s.verify().is_err() || m.schema > SCHEMA || !shareable_verdict(&m.verdict) || !m.delta_pct.is_finite() {
            continue;
        }
        latest.insert((s.pubkey.clone(), m.game_id.clone(), m.hw.class.clone(), m.tweak_id.clone()), m);
    }
    let mut groups: BTreeMap<(String, String, String), Vec<&Measurement>> = BTreeMap::new();
    for m in latest.values() {
        let tier = format!("tier:{}:{}:{}", m.hw.tier, if m.hw.laptop { "laptop" } else { "desktop" }, m.hw.gpu_vendor);
        for scope in [format!("class:{}", m.hw.class), tier] {
            groups.entry((m.game_id.clone(), m.tweak_id.clone(), scope)).or_default().push(m);
        }
    }
    let presets = groups
        .into_iter()
        .map(|((game_id, tweak_id, scope), ms)| {
            let n = ms.len();
            let kept = ms.iter().filter(|m| m.verdict == "kept").count();
            let harmful = ms.iter().filter(|m| m.verdict == "harmful").count();
            let med = median(ms.iter().map(|m| m.delta_pct).collect());
            let enough = n >= rules.min_installs;
            let status = if enough && kept as f64 >= rules.agreement * n as f64 && med >= rules.min_gain_pct {
                PresetStatus::Verified
            } else if enough && harmful as f64 >= rules.agreement * n as f64 && med <= -rules.min_gain_pct {
                PresetStatus::Harmful
            } else {
                PresetStatus::Pending
            };
            Preset { game_id, tweak_id, scope, installs: n, kept, harmful, median_delta_pct: med, status }
        })
        .collect();
    PresetFile { schema: SCHEMA, generated_day: day.into(), min_installs: rules.min_installs, presets }
}

// ------------------------------------------------------------------ help a friend

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HelpHw {
    pub tier: u8,
    pub laptop: bool,
    /// Commercial model names (e.g. "Intel Core i5-11400F", "NVIDIA GeForce GTX 1660 SUPER"):
    /// the friend needs them to advise, and they identify a product, not a person.
    pub cpu: String,
    pub gpus: Vec<String>,
    pub ram_gb: u32,
    pub ram_mts: Option<u32>,
    pub ram_rated_mts: Option<u32>,
    pub windows_build: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HelpTweak {
    pub id: String,
    pub enabled: bool,
    /// untested | kept | no_benefit | harmful
    pub history: String,
    pub lock: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HelpGame {
    pub id: String,
    pub name: String,
    /// none | user_mode | kernel
    pub anticheat: String,
    pub baseline_runs: usize,
    pub tweaks: Vec<HelpTweak>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HelpRequest {
    pub schema: u32,
    pub app: String,
    pub hw: HelpHw,
    /// Structural issue ids (e.g. `ram_below_rated`).
    pub issues: Vec<String>,
    pub games: Vec<HelpGame>,
}

impl Versioned for HelpRequest {
    fn schema(&self) -> u32 {
        self.schema
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HelpReco {
    pub game_id: String,
    pub enable: Vec<String>,
    pub disable: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HelpResponse {
    pub schema: u32,
    pub app: String,
    /// Which request this answers (`short_id` of the signed request).
    pub request: String,
    pub games: Vec<HelpReco>,
    /// Plain text from the friend, max 280 characters.
    pub note: String,
}

impl Versioned for HelpResponse {
    fn schema(&self) -> u32 {
        self.schema
    }
}

pub const HELP_REQUEST_PREFIX: &str = "ODH1";
pub const HELP_RESPONSE_PREFIX: &str = "ODR1";

impl HelpResponse {
    /// Keep only recommendations about games and tweaks the request contained, unlocked
    /// ones only: a friend can toggle what the plan already offers, never add anything.
    pub fn restricted_to(mut self, req: &HelpRequest) -> Self {
        self.note = self.note.chars().take(280).collect();
        self.games.retain(|g| req.games.iter().any(|r| r.id == g.game_id));
        for g in &mut self.games {
            let rg = req.games.iter().find(|r| r.id == g.game_id).expect("retained above");
            let ok = |t: &String| rg.tweaks.iter().any(|x| &x.id == t && x.lock.is_none());
            g.enable.retain(ok);
            g.disable.retain(ok);
            g.enable.retain(|t| !g.disable.contains(t));
        }
        self
    }
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeSource;

    fn id(n: u8) -> Identity {
        Identity::from_seed([n; 32])
    }

    fn m(game: &str, class: &str, tweak: &str, verdict: &str, delta: f64) -> Measurement {
        Measurement {
            schema: SCHEMA,
            app: "1.0.0".into(),
            game_id: game.into(),
            hw: HwClass { tier: 2, laptop: false, gpu_vendor: "nvidia".into(), class: class.into() },
            tweak_id: tweak.into(),
            verdict: verdict.into(),
            delta_pct: delta,
            ci_low: delta - 2.0,
            ci_high: delta + 2.0,
            runs: 4,
        }
    }

    #[test]
    fn signatures_catch_any_edit() {
        let s = id(1).sign(&m("steam:1", "c", "game_mode", "kept", 7.0));
        assert!(s.verify().is_ok());
        let code = encode(MEASUREMENT_PREFIX, &s);
        let back: Signed<Measurement> = open(MEASUREMENT_PREFIX, &code).unwrap();
        assert_eq!(back, s);
        let mut forged = s.clone();
        forged.payload.delta_pct = 40.0;
        assert_eq!(forged.verify(), Err(ShareError::BadSignature));
        let mut other_key = s.clone();
        other_key.pubkey = id(2).public_hex();
        assert_eq!(other_key.verify(), Err(ShareError::BadSignature));
        assert_eq!(decode::<Signed<Measurement>>(CARD_PREFIX, &code), Err(ShareError::BadCode));
        assert_eq!(decode::<Signed<Measurement>>(MEASUREMENT_PREFIX, "ODM1.!!!"), Err(ShareError::BadCode));
    }

    #[test]
    fn zip_bombs_and_huge_codes_are_refused() {
        let bomb = miniz_oxide::deflate::compress_to_vec(&vec![b' '; 10 * 1024 * 1024], 9);
        let code = format!("ODM1.{}", b64().encode(bomb));
        assert!(code.len() < MAX_CODE);
        assert_eq!(decode::<serde_json::Value>(MEASUREMENT_PREFIX, &code), Err(ShareError::BadCode));
        assert_eq!(decode::<serde_json::Value>(MEASUREMENT_PREFIX, &format!("ODM1.{}", "A".repeat(MAX_CODE))), Err(ShareError::BadCode));
    }

    #[test]
    fn hardware_class_reveals_no_model_and_is_stable() {
        let p = crate::build_profile(&FakeSource::laptop_hybrid());
        let a = hw_class(&p);
        assert_eq!(a, hw_class(&p));
        assert_eq!(a.class.len(), 16);
        let json = serde_json::to_string(&a).unwrap().to_ascii_lowercase();
        for g in &p.gpus {
            assert!(!json.contains(&g.name.to_ascii_lowercase()), "{json}");
        }
        assert!(!json.contains(&p.cpu.name.value.clone().unwrap().to_ascii_lowercase()));
        assert_ne!(a.class, hw_class(&crate::build_profile(&FakeSource::desktop_entry())).class);
    }

    #[test]
    fn cards_need_measured_data_and_never_hide_a_stutter() {
        let p = crate::build_profile(&FakeSource::desktop_entry());
        let mut series: Vec<f64> = vec![8.3; 5000];
        series[2500] = 95.0; // one hitch
        fn input<'a>(p: &HardwareProfile, runs: usize, s: &'a [f64]) -> CardInput<'a> {
            CardInput {
            app: "1.0.0",
            game_id: "steam:1",
            game: "Game",
            hw: hw_class(p),
            bottleneck: "cpu_bound",
            baseline_runs: runs,
            min_runs: 3,
            baseline: CardMetrics { avg_fps: 110.04, low1: 61.27, stdev_ms: 3.1 },
            optimized: CardMetrics { avg_fps: 118.0, low1: 79.0, stdev_ms: 1.9 },
            baseline_series: s,
            optimized_series: s,
            kept: vec!["power_session".into(), "game_mode".into()],
            day: "2026-11-16T21:13:00Z",
            }
        }
        assert_eq!(make_card(input(&p, 2, &series)), Err(ShareError::NotMeasured));
        assert_eq!(make_card(input(&p, 3, &[])), Err(ShareError::NotMeasured));
        let c = make_card(input(&p, 3, &series)).unwrap();
        assert_eq!(c.baseline_series.len(), CARD_POINTS);
        assert!(c.baseline_series.contains(&95.0), "the hitch survives downsampling");
        assert_eq!(c.day, "2026-11-16");
        assert_eq!(c.kept, vec!["game_mode", "power_session"]);
        assert_eq!(c.low1_delta_pct(), 28.9);
        let signed = id(3).sign(&c);
        let back: Signed<Card> = open(CARD_PREFIX, &encode(CARD_PREFIX, &signed)).unwrap();
        assert_eq!(back.short_id(), signed.short_id());
    }

    #[test]
    fn presets_need_independent_agreement() {
        let rules = AggregateRules::default();
        let mut items = Vec::new();
        // One install submitting 10 times counts once.
        for _ in 0..10 {
            items.push(id(1).sign(&m("steam:1", "c1", "game_mode", "kept", 9.0)));
        }
        let f = aggregate(&items, &rules, "2026-11-16");
        let p = f.presets.iter().find(|p| p.scope == "class:c1").unwrap();
        assert_eq!((p.installs, p.status), (1, PresetStatus::Pending));

        // 5 independent installs, 4 kept + 1 no_benefit = 80 % → verified.
        for n in 2..=5 {
            items.push(id(n).sign(&m("steam:1", "c1", "game_mode", if n == 5 { "no_benefit" } else { "kept" }, 6.0 + n as f64)));
        }
        let f = aggregate(&items, &rules, "d");
        let p = f.presets.iter().find(|p| p.scope == "class:c1").unwrap();
        assert_eq!((p.installs, p.kept, p.status), (5, 4, PresetStatus::Verified));
        assert_eq!(p.median_delta_pct, 9.0);
        // The same data also counts for the coarse tier scope.
        assert!(f.presets.iter().any(|p| p.scope == "tier:2:desktop:nvidia" && p.installs == 5));

        // Forged and inconclusive submissions are ignored.
        let mut forged = id(6).sign(&m("steam:1", "c1", "game_mode", "kept", 50.0));
        forged.payload.delta_pct = 90.0;
        items.push(forged);
        items.push(id(7).sign(&m("steam:1", "c1", "game_mode", "inconclusive", 0.0)));
        let p2 = aggregate(&items, &rules, "d").presets.into_iter().find(|p| p.scope == "class:c1").unwrap();
        assert_eq!(p2.installs, 5);

        // Mixed results never become a claim.
        let mixed: Vec<_> = (10..20)
            .map(|n| id(n).sign(&m("steam:2", "c2", "hags", if n % 2 == 0 { "kept" } else { "harmful" }, if n % 2 == 0 { 8.0 } else { -8.0 })))
            .collect();
        let f = aggregate(&mixed, &rules, "d");
        assert!(f.presets.iter().all(|p| p.status == PresetStatus::Pending));
    }

    #[test]
    fn issue_urls_carry_the_code_and_the_aggregator_finds_it() {
        let s = vec![id(1).sign(&m("steam:1", "c", "game_mode", "kept", 7.0))];
        let url = issue_url("owner/repo", &s);
        assert!(url.starts_with("https://github.com/owner/repo/issues/new?title=%5Boverdrive-measurement%5D"));
        let body = format!("text\n```\n{}\n```\n", encode(MEASUREMENT_PREFIX, &s));
        let codes = codes_in(&body);
        assert_eq!(codes.len(), 1);
        let back: Vec<Signed<Measurement>> = decode(MEASUREMENT_PREFIX, &codes[0]).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn a_friend_can_only_toggle_what_the_plan_offers() {
        let req = HelpRequest {
            schema: SCHEMA,
            app: "1".into(),
            hw: HelpHw { tier: 1, laptop: true, cpu: "x".into(), gpus: vec![], ram_gb: 8, ram_mts: None, ram_rated_mts: None, windows_build: None },
            issues: vec![],
            games: vec![HelpGame {
                id: "steam:1".into(),
                name: "G".into(),
                anticheat: "kernel".into(),
                baseline_runs: 3,
                tweaks: vec![
                    HelpTweak { id: "game_mode".into(), enabled: true, history: "untested".into(), lock: None },
                    HelpTweak { id: "game_config".into(), enabled: false, history: "untested".into(), lock: Some("anticheat".into()) },
                ],
            }],
        };
        let resp = HelpResponse {
            schema: SCHEMA,
            app: "1".into(),
            request: "abcd1234".into(),
            games: vec![
                HelpReco { game_id: "steam:1".into(), enable: vec!["game_config".into(), "made_up".into(), "game_mode".into()], disable: vec![] },
                HelpReco { game_id: "steam:999".into(), enable: vec!["game_mode".into()], disable: vec![] },
            ],
            note: "x".repeat(1000),
        }
        .restricted_to(&req);
        assert_eq!(resp.games.len(), 1);
        assert_eq!(resp.games[0].enable, vec!["game_mode"], "locked and unknown tweaks are dropped");
        assert_eq!(resp.note.chars().count(), 280);
    }
}
