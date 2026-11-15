//! Module I — Drift Guardian (goals.md §5.8).
//!
//! Windows feature updates, GPU driver updates and OEM utilities silently reset settings.
//! The guardian re-checks every accepted persistent change, repairs low-risk drift on its
//! own (asks once otherwise), and spots performance regressions with their likely cause.

use serde::Serialize;
use serde_json::{json, Value};

use crate::catalog::{Catalog, Risk};
use crate::engine::journal::{Journal, StepState, TxKind, TxState};
use crate::engine::tx::{Engine, PlannedAction, TxError};
use crate::engine::{actions, SystemOps};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    WindowsUpdate,
    DriverUpdate,
    Boot,
    Daily,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "cause",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Cause {
    DriverChanged {
        from: Option<String>,
        to: Option<String>,
    },
    GameSettingsChanged,
    PowerSourceChanged,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "finding",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Finding {
    /// An accepted persistent change is no longer in place.
    Reverted {
        tx_id: i64,
        step_id: i64,
        tweak_id: String,
        kind: String,
    },
    /// Recent sessions are clearly worse than before.
    Regression {
        game_id: String,
        drop_pct: f64,
        cause: Cause,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RepairOutcome {
    Repaired {
        new_tx: i64,
    },
    /// Not low-risk: ask the user once, in plain language.
    NeedsConsent {
        tweak_id: String,
    },
    NotRepairable,
}

/// Regression threshold (% drop of the 1 % low with the same configuration).
pub const REGRESSION_PCT: f64 = 10.0;

pub struct DriftGuardian<'a> {
    pub journal: &'a Journal,
    pub sys: &'a dyn SystemOps,
    pub catalog: &'a Catalog,
}

impl DriftGuardian<'_> {
    /// Re-verify every accepted persistent change.
    pub fn check(&self, _trigger: Trigger) -> Result<Vec<Finding>, TxError> {
        let mut out = Vec::new();
        for t in self
            .journal
            .txs_in(&[TxState::Applied])?
            .into_iter()
            .filter(|t| t.kind == TxKind::Persistent)
        {
            for s in self.journal.steps(t.tx_id)? {
                if s.state == StepState::Verified
                    && !actions::verify_applied(&s.action, self.sys).unwrap_or(false)
                {
                    out.push(Finding::Reverted {
                        tx_id: t.tx_id,
                        step_id: s.step_id,
                        tweak_id: s.tweak_id.clone(),
                        kind: s.action.kind().into(),
                    });
                }
            }
        }
        Ok(out)
    }

    /// Re-apply a drifted change. Low-risk changes the user already accepted are repaired
    /// automatically; anything else needs `approved = true`.
    pub fn repair(&self, f: &Finding, approved: bool) -> Result<RepairOutcome, TxError> {
        let Finding::Reverted {
            tx_id,
            step_id,
            tweak_id,
            ..
        } = f
        else {
            return Ok(RepairOutcome::NotRepairable);
        };
        let low = self
            .catalog
            .get(tweak_id)
            .is_some_and(|m| m.risk == Risk::Low);
        if !low && !approved {
            return Ok(RepairOutcome::NeedsConsent {
                tweak_id: tweak_id.clone(),
            });
        }
        let step = self
            .journal
            .steps(*tx_id)?
            .into_iter()
            .find(|s| s.step_id == *step_id)
            .ok_or(TxError::NotFound(*tx_id))?;
        let game = self.journal.tx(*tx_id)?.and_then(|t| t.game_id);
        // The old step no longer describes reality; the repair is a fresh, journaled change.
        self.journal.mark(
            *step_id,
            StepState::Superseded,
            Some((
                "DRIFT_REPAIRED",
                "reset by an update; re-applied in a new transaction",
            )),
        )?;
        let reason = json!({ "driftRepair": tweak_id }).to_string();
        let new_tx = Engine::new(self.journal, self.sys).apply(
            TxKind::Persistent,
            game.as_deref(),
            &reason,
            &[PlannedAction::new(tweak_id, step.action)],
        )?;
        Ok(RepairOutcome::Repaired { new_tx })
    }

    /// Compare the latest sessions of one configuration with the earlier ones.
    pub fn regression(
        &self,
        game_id: &str,
        tweak_set: &[String],
    ) -> Result<Option<Finding>, TxError> {
        let mut key = tweak_set.to_vec();
        key.sort();
        let same: Vec<_> = self
            .journal
            .sessions(game_id)?
            .into_iter()
            .filter(|s| s.valid && s.tweak_set == key)
            .collect();
        if same.len() < 5 {
            return Ok(None);
        }
        let (earlier, recent) = same.split_at(same.len() - 3);
        let mean = |v: &[crate::engine::journal::SessionRow]| {
            v.iter().map(|s| s.metrics.low_1).sum::<f64>() / v.len() as f64
        };
        let drop = (mean(recent) / mean(earlier) - 1.0) * 100.0;
        if drop > -REGRESSION_PCT {
            return Ok(None);
        }
        let (a, b) = (
            &earlier.last().unwrap().context,
            &recent.last().unwrap().context,
        );
        let field = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(String::from);
        let cause = if field(a, "driverVersion") != field(b, "driverVersion") {
            Cause::DriverChanged {
                from: field(a, "driverVersion"),
                to: field(b, "driverVersion"),
            }
        } else if field(a, "settingsHash") != field(b, "settingsHash") {
            Cause::GameSettingsChanged
        } else if field(a, "powerSource") != field(b, "powerSource") {
            Cause::PowerSourceChanged
        } else {
            Cause::Unknown
        };
        Ok(Some(Finding::Regression {
            game_id: game_id.into(),
            drop_pct: (drop * 10.0).round() / 10.0,
            cause,
        }))
    }
}
