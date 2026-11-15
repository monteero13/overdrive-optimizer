//! Write-ahead journal on SQLite (WAL, synchronous=FULL). The schema is
//! `docs/architecture/journal.sql` — included verbatim, single source of truth.
//!
//! Every write is its own committed statement, so the order on disk is the order of
//! events: a step's `before` snapshot is durable *before* the change is applied.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use super::{actions, Action};

const SCHEMA: &str = include_str!("../../../../docs/architecture/journal.sql");

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("sqlite: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("corrupt journal row: {0}")]
    Corrupt(String),
}

pub type JResult<T> = Result<T, JournalError>;

macro_rules! str_enum {
    ($name:ident { $($v:ident = $s:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($v),+ }
        impl $name {
            pub fn as_str(self) -> &'static str { match self { $(Self::$v => $s),+ } }
            pub fn parse(s: &str) -> JResult<Self> {
                match s { $($s => Ok(Self::$v),)+ _ => Err(JournalError::Corrupt(format!("{}: {s}", stringify!($name)))) }
            }
        }
    };
}

str_enum!(TxKind { Session = "session", Persistent = "persistent", GlobalReset = "global_reset", ForeignRestore = "foreign_restore" });
str_enum!(TxState { Open = "open", Applying = "applying", Applied = "applied", Reverting = "reverting", Reverted = "reverted", Failed = "failed", Abandoned = "abandoned" });
str_enum!(StepState {
    Planned = "planned",
    Captured = "captured",
    Applied = "applied",
    Verified = "verified",
    ApplyFailed = "apply_failed",
    Reverted = "reverted",
    RevertVerified = "revert_verified",
    RevertFailed = "revert_failed",
    Superseded = "superseded",
});

#[derive(Debug, Clone, PartialEq)]
pub struct TxRow {
    pub tx_id: i64,
    pub kind: TxKind,
    pub state: TxState,
    pub game_id: Option<String>,
    pub reason: Option<String>,
    pub created_at: i64,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepRow {
    pub step_id: i64,
    pub tx_id: i64,
    pub seq: i64,
    pub tweak_id: String,
    pub action: Action,
    pub before: Option<Value>,
    pub state: StepState,
    pub error_code: Option<String>,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn random_id() -> String {
    // No RNG dependency: time + pid + address entropy is plenty for a per-boot id.
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let x = &t as *const _ as usize;
    format!("{:x}-{:x}-{:x}", t, std::process::id(), x)
}

/// Raw `transactions` row: (tx_id, kind, state, game_id, reason, created_at, error_code).
type RawTx = (
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
    Option<String>,
);

pub struct Journal {
    conn: Connection,
    boot_id: String,
}

impl Journal {
    pub fn open(path: &Path, svc_version: &str) -> JResult<Self> {
        Self::init(Connection::open(path)?, svc_version)
    }

    pub fn open_in_memory(svc_version: &str) -> JResult<Self> {
        Self::init(Connection::open_in_memory()?, svc_version)
    }

    fn init(conn: Connection, svc_version: &str) -> JResult<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        conn.execute(
            "INSERT OR IGNORE INTO meta(key, value) VALUES ('schema_version', '1')",
            [],
        )?;
        let boot_id = random_id();
        conn.execute(
            "INSERT INTO boots(boot_id, started_at, svc_version) VALUES (?1, ?2, ?3)",
            params![boot_id, now_ms(), svc_version],
        )?;
        Ok(Self { conn, boot_id })
    }

    pub fn boot_id(&self) -> &str {
        &self.boot_id
    }

    pub fn mark_boot_recovered(&self) -> JResult<()> {
        self.conn.execute(
            "UPDATE boots SET recovered = 1 WHERE boot_id = ?1",
            params![self.boot_id],
        )?;
        Ok(())
    }

    pub fn begin(&self, kind: TxKind, game_id: Option<&str>, reason: &str) -> JResult<i64> {
        self.conn.execute(
            "INSERT INTO transactions(kind, state, game_id, boot_id, reason, created_at) VALUES (?1, 'open', ?2, ?3, ?4, ?5)",
            params![kind.as_str(), game_id, self.boot_id, reason, now_ms()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn add_step(
        &self,
        tx: i64,
        seq: i64,
        tweak_id: &str,
        tweak_version: i64,
        action: &Action,
    ) -> JResult<i64> {
        let target =
            serde_json::to_string(action).map_err(|e| JournalError::Corrupt(e.to_string()))?;
        let after = actions::intended(action).to_string();
        self.conn.execute(
            "INSERT INTO steps(tx_id, seq, tweak_id, tweak_version, action_kind, target_json, after_json, state, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'planned', ?8)",
            params![tx, seq, tweak_id, tweak_version, action.kind(), target, after, now_ms()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The write-ahead point: after this returns, the original state is durable.
    pub fn record_before(&self, step: i64, before: &Value) -> JResult<()> {
        self.conn.execute(
            "UPDATE steps SET before_json = ?2, state = 'captured', updated_at = ?3 WHERE step_id = ?1",
            params![step, before.to_string(), now_ms()],
        )?;
        Ok(())
    }

    pub fn mark(&self, step: i64, state: StepState, error: Option<(&str, &str)>) -> JResult<()> {
        self.conn.execute(
            "UPDATE steps SET state = ?2, attempts = attempts + 1, error_code = ?3, error_detail = ?4, updated_at = ?5 WHERE step_id = ?1",
            params![step, state.as_str(), error.map(|e| e.0), error.map(|e| e.1), now_ms()],
        )?;
        Ok(())
    }

    pub fn set_tx(&self, tx: i64, state: TxState, error: Option<&str>) -> JResult<()> {
        let closed = matches!(
            state,
            TxState::Applied | TxState::Reverted | TxState::Failed | TxState::Abandoned
        )
        .then(now_ms);
        self.conn.execute(
            "UPDATE transactions SET state = ?2, error_code = COALESCE(?3, error_code), closed_at = COALESCE(?4, closed_at) WHERE tx_id = ?1",
            params![tx, state.as_str(), error, closed],
        )?;
        Ok(())
    }

    fn tx_from_row(r: &rusqlite::Row) -> rusqlite::Result<RawTx> {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
        ))
    }

    fn to_tx(t: RawTx) -> JResult<TxRow> {
        Ok(TxRow {
            tx_id: t.0,
            kind: TxKind::parse(&t.1)?,
            state: TxState::parse(&t.2)?,
            game_id: t.3,
            reason: t.4,
            created_at: t.5,
            error_code: t.6,
        })
    }

    const TX_COLS: &'static str = "tx_id, kind, state, game_id, reason, created_at, error_code";

    pub fn tx(&self, tx: i64) -> JResult<Option<TxRow>> {
        let row = self
            .conn
            .query_row(
                &format!(
                    "SELECT {} FROM transactions WHERE tx_id = ?1",
                    Self::TX_COLS
                ),
                params![tx],
                Self::tx_from_row,
            )
            .optional()?;
        row.map(Self::to_tx).transpose()
    }

    /// Transactions in any of `states`, oldest first.
    pub fn txs_in(&self, states: &[TxState]) -> JResult<Vec<TxRow>> {
        let list = states
            .iter()
            .map(|s| format!("'{}'", s.as_str()))
            .collect::<Vec<_>>()
            .join(",");
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM transactions WHERE state IN ({list}) ORDER BY tx_id",
            Self::TX_COLS
        ))?;
        let rows = st
            .query_map([], Self::tx_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter().map(Self::to_tx).collect()
    }

    pub fn steps(&self, tx: i64) -> JResult<Vec<StepRow>> {
        let mut st = self.conn.prepare("SELECT step_id, tx_id, seq, tweak_id, target_json, before_json, state, error_code FROM steps WHERE tx_id = ?1 ORDER BY seq")?;
        let rows = st
            .query_map(params![tx], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, Option<String>>(7)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(
                |(step_id, tx_id, seq, tweak_id, target, before, state, error_code)| {
                    Ok(StepRow {
                        step_id,
                        tx_id,
                        seq,
                        tweak_id,
                        action: serde_json::from_str(&target)
                            .map_err(|e| JournalError::Corrupt(format!("step {step_id}: {e}")))?,
                        before: before
                            .map(|b| serde_json::from_str(&b))
                            .transpose()
                            .map_err(|e| {
                                JournalError::Corrupt(format!("step {step_id} before: {e}"))
                            })?,
                        state: StepState::parse(&state)?,
                        error_code,
                    })
                },
            )
            .collect()
    }
}
