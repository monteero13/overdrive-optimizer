//! Module E — Session Orchestrator (goals.md §5.6).
//!
//! Lifecycle driven by the service:
//! 1. `arm(game)`            — user pressed "Optimize": apply the *before-launch* tweaks of
//!                             this session's configuration (GPU preference, IFEO…).
//! 2. `on_game_start(...)`   — game process appeared: apply the *at-launch* tweaks
//!                             (power plan, background isolation, update pause).
//! 3. `on_game_exit(...)`    — revert everything of the session, compute metrics, record the
//!                             session, let the A/B protocol decide, report and offer to keep
//!                             proven persistent wins.
//! 4. `accept_persistent()`  — the user said "keep it": apply as a persistent transaction.
//!
//! Which tweaks a session uses comes from `experiment::next` (stateless, journal-driven).

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_json::json;

use crate::catalog::{Build, Catalog, GameTarget, Measure, PlanContext, Timing};
use crate::engine::journal::{Journal, StepState, TxKind, TxState};
use crate::engine::tx::{Engine, PlannedAction, RevertReport, TxError};
use crate::engine::{ProcRef, RegRoot, SystemOps};
use crate::experiment::{self, Comparison, ExperimentSettings, Next, SessionPoint, Verdict};
use crate::profile::HardwareProfile;
use crate::safety::{ProcInfo, SafetyPolicy};
use crate::telemetry::classify::{classify, Thresholds};
use crate::telemetry::metrics::{compute, SessionMetrics};
use crate::telemetry::{Bottleneck, FrameSample, SystemSample};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "reason", content = "detail", rename_all = "snake_case")]
pub enum Exclusion {
    NotApplicable(String),
    AlreadyOptimal,
    AntiCheat,
    /// Made this game worse on this machine.
    Blacklisted,
    NoBenefit,
    EasyMode,
    /// Needs a reboot: offered manually (Expert), not auto-tested.
    NeedsReboot,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPlan {
    pub game_id: String,
    /// Measured tweaks active this session (kept so far + the one being tested).
    pub measured: Vec<String>,
    pub testing: Option<String>,
    /// Protective, unmeasured (e.g. pause_updates).
    pub protective: Vec<String>,
    /// Already applied permanently (accepted by the user): nothing to apply per session.
    pub persistent: Vec<String>,
    pub excluded: Vec<(String, Exclusion)>,
    pub converged: bool,
}

#[derive(Debug, Clone)]
pub struct ArmedSession {
    pub plan: SessionPlan,
    pub tx: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ActiveSession {
    pub game_id: String,
    /// Measured tweaks that were really in effect (before-launch ones only if armed in time).
    pub tweak_set: Vec<String>,
    pub testing: Option<String>,
    pub txs: Vec<i64>,
    pub blocked: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionReport {
    pub game_id: String,
    pub session_id: i64,
    pub valid: bool,
    pub metrics: SessionMetrics,
    pub bottleneck: Bottleneck,
    pub bottleneck_confidence: f64,
    pub tweak_set: Vec<String>,
    pub testing: Option<String>,
    /// Verdicts reached thanks to this session.
    pub decisions: Vec<(String, Comparison)>,
    /// Proven persistent wins the user can now choose to keep.
    pub offers: Vec<String>,
    /// Everything the session changed was put back (precise failures otherwise).
    pub reverted: Vec<RevertReport>,
    pub next_testing: Option<String>,
    pub converged: bool,
}

pub struct Orchestrator<'a> {
    pub journal: &'a Journal,
    pub sys: &'a dyn SystemOps,
    pub catalog: &'a Catalog,
    pub settings: ExperimentSettings,
    pub user_root: RegRoot,
    pub capabilities: Vec<String>,
    pub expert: bool,
    pub now_hour: u8,
}

impl<'a> Orchestrator<'a> {
    pub fn new(journal: &'a Journal, sys: &'a dyn SystemOps, catalog: &'a Catalog) -> Self {
        Self {
            journal,
            sys,
            catalog,
            settings: ExperimentSettings::default(),
            user_root: RegRoot::CurrentUser,
            capabilities: vec![],
            expert: false,
            now_hour: 20,
        }
    }

    fn ctx<'b>(
        &'b self,
        game: &'b GameTarget,
        hw: &'b HardwareProfile,
        procs: Option<&'b [ProcInfo]>,
        policy: &'b SafetyPolicy,
    ) -> PlanContext<'b> {
        PlanContext {
            game,
            hw,
            user_root: self.user_root.clone(),
            procs,
            policy,
            capabilities: &self.capabilities,
            now_hour: self.now_hour,
        }
    }

    pub fn verdicts(&self, game_id: &str) -> Result<HashMap<String, Verdict>, TxError> {
        Ok(self
            .journal
            .outcomes(game_id)?
            .into_iter()
            .filter_map(|o| Verdict::parse(&o.verdict).map(|v| (o.tweak_id, v)))
            .collect())
    }

    /// Tweaks currently applied permanently (accepted persistent transactions).
    pub fn persistent_tweaks(&self) -> Result<HashSet<String>, TxError> {
        let mut out = HashSet::new();
        for t in self
            .journal
            .txs_in(&[TxState::Applied])?
            .into_iter()
            .filter(|t| t.kind == TxKind::Persistent)
        {
            for s in self.journal.steps(t.tx_id)? {
                if s.state == StepState::Verified {
                    out.insert(s.tweak_id);
                }
            }
        }
        Ok(out)
    }

    /// Experiment candidates in order + why the others are out.
    pub fn candidates(
        &self,
        game: &GameTarget,
        hw: &HardwareProfile,
    ) -> Result<(Vec<String>, Vec<(String, Exclusion)>), TxError> {
        let verdicts = self.verdicts(&game.id)?;
        let policy = SafetyPolicy::default();
        let ctx = self.ctx(game, hw, None, &policy);
        let (mut cands, mut excl) = (Vec::new(), Vec::new());
        for m in self
            .catalog
            .tweaks
            .iter()
            .filter(|m| m.measure != Measure::None)
        {
            let id = m.id.clone();
            let ex = match verdicts.get(&id) {
                Some(Verdict::Harmful) => Some(Exclusion::Blacklisted),
                Some(Verdict::NoBenefit) => Some(Exclusion::NoBenefit),
                _ if m.requires_reboot => Some(Exclusion::NeedsReboot),
                _ if !self.expert && !m.easy_mode_ok() => Some(Exclusion::EasyMode),
                _ if game.anticheat.forbids_process_access() && !m.anticheat_safe => {
                    Some(Exclusion::AntiCheat)
                }
                Some(Verdict::Kept) => None, // stays in the configuration even if now "already optimal" (persistent)
                _ => match self.catalog.build(&id, &ctx, self.sys).map_err(|e| {
                    TxError::StepFailed {
                        tweak_id: id.clone(),
                        error: e.to_string(),
                        rolled_back: false,
                    }
                })? {
                    Build::NotApplicable(r) => Some(Exclusion::NotApplicable(r.into())),
                    Build::AlreadyOptimal => Some(Exclusion::AlreadyOptimal),
                    Build::Actions(_) | Build::Deferred => None,
                },
            };
            match ex {
                Some(e) => excl.push((id, e)),
                None => cands.push(id),
            }
        }
        Ok((cands, excl))
    }

    fn points(&self, game_id: &str) -> Result<Vec<SessionPoint>, TxError> {
        Ok(self
            .journal
            .sessions(game_id)?
            .into_iter()
            .map(|s| SessionPoint {
                tweak_set: s.tweak_set,
                context: s.context,
                metrics: s.metrics,
                valid: s.valid,
            })
            .collect())
    }

    fn measure_of(&self) -> impl Fn(&str) -> Measure + '_ {
        move |id: &str| {
            self.catalog
                .get(id)
                .map(|m| m.measure)
                .unwrap_or(Measure::Frametime)
        }
    }

    /// Record every verdict the data already supports. Returns the new decisions.
    pub fn settle(
        &self,
        game: &GameTarget,
        hw: &HardwareProfile,
    ) -> Result<Vec<(String, Comparison)>, TxError> {
        let mut decisions = Vec::new();
        for _ in 0..self.catalog.tweaks.len() + 1 {
            let (cands, _) = self.candidates(game, hw)?;
            let verdicts = self.verdicts(&game.id)?;
            match experiment::next(
                &cands,
                &verdicts,
                &self.points(&game.id)?,
                &self.measure_of(),
                &self.settings,
            ) {
                Next::Decide {
                    tweak,
                    comparison: c,
                } => {
                    self.journal.set_outcome(
                        &tweak,
                        &game.id,
                        c.verdict.as_str(),
                        c.delta_pct,
                        c.ci_low,
                        c.ci_high,
                        c.runs_candidate as i64,
                    )?;
                    decisions.push((tweak, c));
                }
                _ => break,
            }
        }
        Ok(decisions)
    }

    pub fn plan(&self, game: &GameTarget, hw: &HardwareProfile) -> Result<SessionPlan, TxError> {
        let (cands, excluded) = self.candidates(game, hw)?;
        let verdicts = self.verdicts(&game.id)?;
        let next = experiment::next(
            &cands,
            &verdicts,
            &self.points(&game.id)?,
            &self.measure_of(),
            &self.settings,
        );
        let (measured, testing, converged) = match next {
            Next::Run { tweaks, testing } => (tweaks, testing, false),
            Next::Converged { kept } => (kept, None, true),
            // Not settled yet: settle() first. Be conservative meanwhile: only what's kept.
            Next::Decide { .. } => (
                cands
                    .iter()
                    .filter(|c| verdicts.get(*c) == Some(&Verdict::Kept))
                    .cloned()
                    .collect(),
                None,
                false,
            ),
        };
        let policy = SafetyPolicy::default();
        let ctx = self.ctx(game, hw, None, &policy);
        let protective = self
            .catalog
            .tweaks
            .iter()
            .filter(|m| m.measure == Measure::None && (self.expert || m.easy_mode_ok()))
            .filter(|m| {
                matches!(
                    self.catalog.build(&m.id, &ctx, self.sys),
                    Ok(Build::Actions(_) | Build::Deferred)
                )
            })
            .map(|m| m.id.clone())
            .collect();
        let persistent = self.persistent_tweaks()?;
        Ok(SessionPlan {
            game_id: game.id.clone(),
            persistent: measured
                .iter()
                .filter(|m| persistent.contains(*m))
                .cloned()
                .collect(),
            measured,
            testing,
            protective,
            excluded,
            converged,
        })
    }

    fn apply_timing(
        &self,
        game: &GameTarget,
        hw: &HardwareProfile,
        ids: &[String],
        timing: Timing,
        procs: Option<&[ProcInfo]>,
        policy: &SafetyPolicy,
        phase: &str,
        tweak_set: &[String],
    ) -> Result<(Option<i64>, Vec<(String, String)>), TxError> {
        let ctx = self.ctx(game, hw, procs, policy);
        let mut planned = Vec::new();
        let mut blocked = Vec::new();
        for id in ids {
            let Some(meta) = self.catalog.get(id) else {
                continue;
            };
            if meta.timing != timing {
                continue;
            }
            let build =
                self.catalog
                    .build(id, &ctx, self.sys)
                    .map_err(|e| TxError::StepFailed {
                        tweak_id: id.clone(),
                        error: e.to_string(),
                        rolled_back: false,
                    })?;
            if let Build::Actions(actions) = build {
                let (ok, bad) = policy.filter_plan(actions);
                blocked.extend(
                    bad.into_iter()
                        .map(|(a, r)| (id.clone(), format!("{} {r:?}", a.kind()))),
                );
                planned.extend(ok.into_iter().map(|a| PlannedAction::new(id, a)));
            }
        }
        if planned.is_empty() {
            return Ok((None, blocked));
        }
        let reason = json!({ "phase": phase, "tweakSet": tweak_set }).to_string();
        let tx = Engine::new(self.journal, self.sys)
            .with_guard(policy)
            .apply(TxKind::Session, Some(&game.id), &reason, &planned)?;
        Ok((Some(tx), blocked))
    }

    /// "Optimize" pressed: settle verdicts, plan, apply the before-launch part.
    pub fn arm(&self, game: &GameTarget, hw: &HardwareProfile) -> Result<ArmedSession, TxError> {
        self.settle(game, hw)?;
        let plan = self.plan(game, hw)?;
        let ids: Vec<String> = plan
            .measured
            .iter()
            .filter(|m| !plan.persistent.contains(m))
            .cloned()
            .collect();
        let policy = SafetyPolicy::default();
        let (tx, _) = self.apply_timing(
            game,
            hw,
            &ids,
            Timing::BeforeLaunch,
            None,
            &policy,
            "armed",
            &plan.measured,
        )?;
        Ok(ArmedSession { plan, tx })
    }

    /// The game process appeared. Without a prior `arm`, before-launch tweaks can't take
    /// effect for this run, so they are left out of the measured set (honest attribution).
    pub fn on_game_start(
        &self,
        game: &GameTarget,
        hw: &HardwareProfile,
        armed: Option<ArmedSession>,
        game_proc: &ProcRef,
        procs: &[ProcInfo],
        children: HashSet<u32>,
    ) -> Result<ActiveSession, TxError> {
        let (plan, armed_tx) = match armed {
            Some(a) => (a.plan, a.tx),
            None => (self.plan(game, hw)?, None),
        };
        let was_armed = armed_tx.is_some()
            || plan.measured.iter().all(|m| {
                self.catalog
                    .get(m)
                    .is_none_or(|t| t.timing != Timing::BeforeLaunch || plan.persistent.contains(m))
            });
        let tweak_set: Vec<String> = plan
            .measured
            .iter()
            .filter(|m| {
                was_armed
                    || plan.persistent.contains(*m)
                    || self
                        .catalog
                        .get(m)
                        .is_some_and(|t| t.timing == Timing::AtLaunch)
            })
            .cloned()
            .collect();
        let testing = plan.testing.clone().filter(|t| tweak_set.contains(t));
        let policy = SafetyPolicy {
            game: Some((game_proc.clone(), game.anticheat)),
            game_children: children,
            ..Default::default()
        };
        let mut ids: Vec<String> = tweak_set
            .iter()
            .filter(|m| !plan.persistent.contains(m))
            .cloned()
            .collect();
        ids.extend(plan.protective.iter().cloned());
        let (tx, blocked) = self.apply_timing(
            game,
            hw,
            &ids,
            Timing::AtLaunch,
            Some(procs),
            &policy,
            "launch",
            &tweak_set,
        )?;
        Ok(ActiveSession {
            game_id: game.id.clone(),
            tweak_set,
            testing,
            txs: armed_tx.into_iter().chain(tx).collect(),
            blocked,
        })
    }

    /// The game closed: put everything back, measure, decide, report.
    pub fn on_game_exit(
        &self,
        game: &GameTarget,
        hw: &HardwareProfile,
        active: ActiveSession,
        frames: &[FrameSample],
        samples: &[SystemSample],
        context: &serde_json::Value,
    ) -> Result<SessionReport, TxError> {
        let engine = Engine::new(self.journal, self.sys);
        let mut reverted = Vec::new();
        let mut txs = active.txs.clone();
        // Safety net: any other open session transaction for this game goes too.
        for t in self.journal.txs_for_game(
            &game.id,
            &[TxState::Open, TxState::Applying, TxState::Applied],
        )? {
            if t.kind == TxKind::Session && !txs.contains(&t.tx_id) {
                txs.push(t.tx_id);
            }
        }
        for tx in txs.iter().rev() {
            reverted.push(engine.revert(*tx)?);
        }

        let cls = classify(frames, samples, &Thresholds::default());
        let metrics = compute(frames, &cls.non_gameplay);
        let valid = metrics.gameplay_secs >= self.settings.min_gameplay_secs;
        let label = serde_json::to_value(cls.label)
            .ok()
            .and_then(|v| v.as_str().map(String::from));
        let session_id = self.journal.record_session(
            None,
            &game.id,
            &active.tweak_set,
            context,
            &metrics,
            label.as_deref(),
            valid,
        )?;

        let decisions = self.settle(game, hw)?;
        let persistent = self.persistent_tweaks()?;
        let offers = decisions
            .iter()
            .filter(|(t, c)| {
                c.verdict == Verdict::Kept
                    && self.catalog.get(t).is_some_and(|m| m.persistent_candidate)
                    && !persistent.contains(t)
            })
            .map(|(t, _)| t.clone())
            .collect();
        let next = self.plan(game, hw)?;
        Ok(SessionReport {
            game_id: game.id.clone(),
            session_id,
            valid,
            metrics,
            bottleneck: cls.label,
            bottleneck_confidence: cls.confidence,
            tweak_set: active.tweak_set,
            testing: active.testing,
            decisions,
            offers,
            reverted,
            next_testing: next.testing,
            converged: next.converged,
        })
    }

    /// The user accepted to keep a proven persistent win.
    pub fn accept_persistent(
        &self,
        game: &GameTarget,
        hw: &HardwareProfile,
        tweak_id: &str,
    ) -> Result<i64, TxError> {
        let meta = self
            .catalog
            .get(tweak_id)
            .ok_or_else(|| TxError::StepFailed {
                tweak_id: tweak_id.into(),
                error: "unknown tweak".into(),
                rolled_back: false,
            })?;
        let proven = self.verdicts(&game.id)?.get(tweak_id) == Some(&Verdict::Kept);
        if !meta.persistent_candidate || !proven {
            return Err(TxError::StepFailed {
                tweak_id: tweak_id.into(),
                error: "only proven persistent candidates can be kept".into(),
                rolled_back: false,
            });
        }
        let policy = SafetyPolicy::default();
        let ctx = self.ctx(game, hw, None, &policy);
        let actions =
            match self
                .catalog
                .build(tweak_id, &ctx, self.sys)
                .map_err(|e| TxError::StepFailed {
                    tweak_id: tweak_id.into(),
                    error: e.to_string(),
                    rolled_back: false,
                })? {
                Build::Actions(a) => a,
                _ => {
                    return Err(TxError::StepFailed {
                        tweak_id: tweak_id.into(),
                        error: "nothing to apply".into(),
                        rolled_back: false,
                    })
                }
            };
        let planned: Vec<PlannedAction> = actions
            .into_iter()
            .map(|a| PlannedAction::new(tweak_id, a))
            .collect();
        let reason = json!({ "accepted": tweak_id }).to_string();
        Engine::new(self.journal, self.sys).apply(
            TxKind::Persistent,
            Some(&game.id),
            &reason,
            &planned,
        )
    }
}
