//! The transaction engine. Guarantees (docs/architecture/transaction-engine.md §1):
//! - nothing changes before its original state is durable in the journal;
//! - revert restores exactly what was captured, then verifies it;
//! - a crash/kill at ANY point is undone by `recover()` on the next start;
//! - a value someone else changed after us is never overwritten;
//! - a failure never goes silent: it is journaled and returned.
//!
//! `hook` exposes every crash point so tests can kill the process at each one.

use std::time::{Duration, Instant};

use serde::Serialize;

use super::journal::{JResult, Journal, JournalError, StepRow, StepState, TxKind, TxRow, TxState};
use super::{actions, Action, ProcRef, SystemOps};
use crate::safety::ProcessGuard;

#[derive(Debug, Clone)]
pub struct PlannedAction {
    pub tweak_id: String,
    pub tweak_version: i64,
    pub action: Action,
}

impl PlannedAction {
    pub fn new(tweak_id: &str, action: Action) -> Self {
        Self {
            tweak_id: tweak_id.into(),
            tweak_version: 1,
            action,
        }
    }
}

/// Every point where the process could die. The engine calls `hook` at each one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CrashPoint {
    AfterBegin,
    AfterAddStep(usize),
    AfterCapture(usize),
    AfterApply(usize),
    AfterVerify(usize),
    BeforeCommit,
    AfterRevertStep(usize),
    AfterVerifyRevert(usize),
}

#[derive(Debug, thiserror::Error)]
pub enum TxError {
    #[error("journal: {0}")]
    Journal(#[from] JournalError),
    #[error("the safety policy forbids touching {image} (pid {pid}): {reason}")]
    Forbidden {
        pid: u32,
        image: String,
        reason: String,
    },
    #[error("step '{tweak_id}' failed: {error} (rolled back: {rolled_back})")]
    StepFailed {
        tweak_id: String,
        error: String,
        rolled_back: bool,
    },
    #[error("step '{tweak_id}' did not take effect (rolled back: {rolled_back})")]
    VerifyFailed { tweak_id: String, rolled_back: bool },
    #[error("deadline exceeded (rolled back: {rolled_back})")]
    TimedOut { rolled_back: bool },
    #[error("transaction {0} not found")]
    NotFound(i64),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RevertReport {
    pub tx_id: i64,
    pub reverted: usize,
    /// Steps left alone because someone else changed the value after us.
    pub superseded: Vec<String>,
    /// Steps that could not be reverted (precise, actionable list for the UI).
    pub failed: Vec<(String, String)>,
}

impl RevertReport {
    pub fn ok(&self) -> bool {
        self.failed.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct RecoveryReport {
    pub reverted: Vec<RevertReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct ResetReport {
    pub reverted: Vec<RevertReport>,
    /// Targets that still differ from their original state after the reset (should be empty).
    pub remaining_diffs: Vec<String>,
}

pub struct Engine<'a> {
    pub journal: &'a Journal,
    pub sys: &'a dyn SystemOps,
    pub guard: Option<&'a dyn ProcessGuard>,
    pub hook: Option<Box<dyn Fn(CrashPoint) + 'a>>,
    pub deadline: Option<Duration>,
}

const REVERT_ATTEMPTS: usize = 3;

/// Step states that may have changed the system and therefore must be reverted.
fn needs_revert(s: StepState) -> bool {
    matches!(
        s,
        StepState::Captured
            | StepState::Applied
            | StepState::Verified
            | StepState::ApplyFailed
            | StepState::RevertFailed
            | StepState::Reverted
    )
}

impl<'a> Engine<'a> {
    pub fn new(journal: &'a Journal, sys: &'a dyn SystemOps) -> Self {
        Self {
            journal,
            sys,
            guard: None,
            hook: None,
            deadline: None,
        }
    }

    pub fn with_guard(mut self, g: &'a dyn ProcessGuard) -> Self {
        self.guard = Some(g);
        self
    }

    fn at(&self, p: CrashPoint) {
        if let Some(h) = &self.hook {
            h(p);
        }
    }

    /// Apply a plan atomically: all steps verified, or everything rolled back.
    pub fn apply(
        &self,
        kind: TxKind,
        game_id: Option<&str>,
        reason: &str,
        plan: &[PlannedAction],
    ) -> Result<i64, TxError> {
        // Safety first, before anything is journaled or touched.
        if let Some(g) = self.guard {
            for pa in plan {
                if let Some(p) = pa.action.process() {
                    g.check(p).map_err(|reason| TxError::Forbidden {
                        pid: p.pid,
                        image: p.image.clone(),
                        reason,
                    })?;
                }
            }
        }
        let started = Instant::now();
        let tx = self.journal.begin(kind, game_id, reason)?;
        self.at(CrashPoint::AfterBegin);
        self.journal.set_tx(tx, TxState::Applying, None)?;

        for (i, pa) in plan.iter().enumerate() {
            let step =
                self.journal
                    .add_step(tx, i as i64, &pa.tweak_id, pa.tweak_version, &pa.action)?;
            self.at(CrashPoint::AfterAddStep(i));

            let before = match actions::capture(&pa.action, self.sys) {
                Ok(b) => b,
                Err(e) => {
                    self.journal.mark(
                        step,
                        StepState::ApplyFailed,
                        Some(("CAPTURE", &e.to_string())),
                    )?;
                    let rb = self.rollback(tx)?;
                    return Err(TxError::StepFailed {
                        tweak_id: pa.tweak_id.clone(),
                        error: e.to_string(),
                        rolled_back: rb,
                    });
                }
            };
            self.journal.record_before(step, &before)?; // ← write-ahead point
            self.at(CrashPoint::AfterCapture(i));

            if let Err(e) = actions::apply(&pa.action, self.sys) {
                self.journal.mark(
                    step,
                    StepState::ApplyFailed,
                    Some(("APPLY", &e.to_string())),
                )?;
                let rb = self.rollback(tx)?;
                return Err(TxError::StepFailed {
                    tweak_id: pa.tweak_id.clone(),
                    error: e.to_string(),
                    rolled_back: rb,
                });
            }
            self.journal.mark(step, StepState::Applied, None)?;
            self.at(CrashPoint::AfterApply(i));

            match actions::verify_applied(&pa.action, self.sys) {
                Ok(true) => self.journal.mark(step, StepState::Verified, None)?,
                Ok(false) | Err(_) => {
                    self.journal.mark(
                        step,
                        StepState::ApplyFailed,
                        Some(("VERIFY", "state after apply differs from intended")),
                    )?;
                    let rb = self.rollback(tx)?;
                    return Err(TxError::VerifyFailed {
                        tweak_id: pa.tweak_id.clone(),
                        rolled_back: rb,
                    });
                }
            }
            self.at(CrashPoint::AfterVerify(i));

            if self.deadline.is_some_and(|d| started.elapsed() > d) {
                let rb = self.rollback(tx)?;
                return Err(TxError::TimedOut { rolled_back: rb });
            }
        }
        self.at(CrashPoint::BeforeCommit);
        self.journal.set_tx(tx, TxState::Applied, None)?;
        Ok(tx)
    }

    fn rollback(&self, tx: i64) -> JResult<bool> {
        let r = self.revert_steps(tx, |_| true)?;
        self.journal.set_tx(
            tx,
            if r.ok() {
                TxState::Reverted
            } else {
                TxState::Failed
            },
            Some("ROLLED_BACK"),
        )?;
        Ok(r.ok())
    }

    /// Revert a whole transaction (session end, user "undo", recovery).
    pub fn revert(&self, tx: i64) -> Result<RevertReport, TxError> {
        self.journal.tx(tx)?.ok_or(TxError::NotFound(tx))?;
        self.journal.set_tx(tx, TxState::Reverting, None)?;
        let r = self.revert_steps(tx, |_| true)?;
        self.journal.set_tx(
            tx,
            if r.ok() {
                TxState::Reverted
            } else {
                TxState::Failed
            },
            (!r.ok()).then_some("REVERT_FAILED"),
        )?;
        Ok(r)
    }

    fn revert_steps(&self, tx: i64, filter: impl Fn(&StepRow) -> bool) -> JResult<RevertReport> {
        let mut rep = RevertReport {
            tx_id: tx,
            reverted: 0,
            superseded: vec![],
            failed: vec![],
        };
        let steps = self.journal.steps(tx)?;
        for (i, s) in steps.iter().enumerate().rev() {
            if !needs_revert(s.state) || !filter(s) {
                continue;
            }
            let Some(before) = &s.before else { continue };
            // Applied and verified by us: if the target no longer holds our value, someone
            // else changed it after us — respect their choice.
            if matches!(s.state, StepState::Applied | StepState::Verified)
                && actions::changed_by_other(&s.action, self.sys).unwrap_or(false)
            {
                self.journal.mark(
                    s.step_id,
                    StepState::Superseded,
                    Some((
                        "CHANGED_BY_OTHER",
                        "value changed after Overdrive applied it; left as is",
                    )),
                )?;
                rep.superseded.push(s.tweak_id.clone());
                continue;
            }
            let mut last_err = None;
            for _ in 0..REVERT_ATTEMPTS {
                match actions::revert(&s.action, self.sys, before) {
                    Ok(()) => {
                        last_err = None;
                        break;
                    }
                    Err(e) => last_err = Some(e),
                }
            }
            if let Some(e) = last_err {
                self.journal.mark(
                    s.step_id,
                    StepState::RevertFailed,
                    Some(("REVERT", &e.to_string())),
                )?;
                rep.failed.push((s.tweak_id.clone(), e.to_string()));
                continue;
            }
            self.journal.mark(s.step_id, StepState::Reverted, None)?;
            self.at(CrashPoint::AfterRevertStep(i));
            match actions::verify_reverted(&s.action, self.sys, before) {
                Ok(true) => {
                    self.journal
                        .mark(s.step_id, StepState::RevertVerified, None)?;
                    rep.reverted += 1;
                }
                Ok(false) => {
                    self.journal.mark(
                        s.step_id,
                        StepState::RevertFailed,
                        Some(("VERIFY_REVERT", "state after revert differs from original")),
                    )?;
                    rep.failed.push((
                        s.tweak_id.clone(),
                        "state after revert differs from original".into(),
                    ));
                }
                Err(e) => {
                    self.journal.mark(
                        s.step_id,
                        StepState::RevertFailed,
                        Some(("VERIFY_REVERT", &e.to_string())),
                    )?;
                    rep.failed.push((s.tweak_id.clone(), e.to_string()));
                }
            }
            self.at(CrashPoint::AfterVerifyRevert(i));
        }
        Ok(rep)
    }

    /// Run on every service start, BEFORE accepting IPC. Reverts:
    /// - every session transaction that was still open (crash, kill, power loss);
    /// - persistent transactions that were caught half-applied (all or nothing).
    ///
    /// Fully applied persistent changes are kept: the user chose them.
    pub fn recover(&self) -> Result<RecoveryReport, TxError> {
        let mut rep = RecoveryReport::default();
        for t in self.journal.txs_in(&[
            TxState::Open,
            TxState::Applying,
            TxState::Applied,
            TxState::Reverting,
            TxState::Failed,
        ])? {
            let keep = t.kind == TxKind::Persistent && t.state == TxState::Applied;
            let done_failed = t.state == TxState::Failed
                && !self
                    .journal
                    .steps(t.tx_id)?
                    .iter()
                    .any(|s| s.state == StepState::RevertFailed);
            if keep || done_failed || t.kind == TxKind::GlobalReset {
                continue;
            }
            rep.reverted.push(self.revert(t.tx_id)?);
        }
        if !rep.reverted.is_empty() {
            self.journal.mark_boot_recovered()?;
        }
        Ok(rep)
    }

    /// "Put my PC back as it was": every open session, then every persistent change,
    /// newest first; then re-check every target against its original snapshot.
    pub fn revert_all(&self) -> Result<ResetReport, TxError> {
        let marker = self
            .journal
            .begin(TxKind::GlobalReset, None, "user_reset")?;
        let mut rep = ResetReport::default();
        let mut txs: Vec<TxRow> = self.journal.txs_in(&[
            TxState::Open,
            TxState::Applying,
            TxState::Applied,
            TxState::Reverting,
            TxState::Failed,
        ])?;
        txs.retain(|t| t.kind != TxKind::GlobalReset);
        // Sessions first, then persistent newest-first.
        txs.sort_by_key(|t| (t.kind != TxKind::Session, std::cmp::Reverse(t.tx_id)));
        for t in &txs {
            rep.reverted.push(self.revert(t.tx_id)?);
        }
        for t in &txs {
            for s in self.journal.steps(t.tx_id)? {
                if s.state == StepState::RevertVerified {
                    if let Some(b) = &s.before {
                        if !actions::verify_reverted(&s.action, self.sys, b).unwrap_or(false) {
                            rep.remaining_diffs.push(format!(
                                "{} ({})",
                                s.tweak_id,
                                s.action.kind()
                            ));
                        }
                    }
                }
            }
        }
        let clean = rep.reverted.iter().all(RevertReport::ok) && rep.remaining_diffs.is_empty();
        self.journal.set_tx(
            marker,
            if clean {
                TxState::Applied
            } else {
                TxState::Failed
            },
            (!clean).then_some("RESET_INCOMPLETE"),
        )?;
        Ok(rep)
    }

    /// A suspended/lowered process came to the foreground (or the user pressed
    /// "Resume everything" with `p = None`): undo our process actions on it right now.
    pub fn release_processes(&self, p: Option<&ProcRef>) -> Result<usize, TxError> {
        let mut n = 0;
        for t in self
            .journal
            .txs_in(&[TxState::Applied, TxState::Applying])?
        {
            let r = self.revert_steps(t.tx_id, |s| {
                s.action
                    .process()
                    .is_some_and(|sp| p.is_none_or(|p| p == sp))
            })?;
            n += r.reverted;
        }
        Ok(n)
    }
}
