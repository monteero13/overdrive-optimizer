-- Overdrive journal — schema v1
-- SQLite in WAL mode. Opened by overdrive-svc only (LocalSystem, file ACL: SYSTEM + Administrators).
-- PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA foreign_keys = ON;
-- Write-ahead rule: a step row in state 'captured' (with before_json) is COMMITTED before the change is applied.

CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
-- rows: schema_version, machine_id (random, local only), created_at

CREATE TABLE IF NOT EXISTS boots (
  boot_id     TEXT PRIMARY KEY,          -- random per service start
  started_at  INTEGER NOT NULL,          -- unix ms
  svc_version TEXT NOT NULL,
  recovered   INTEGER NOT NULL DEFAULT 0 -- 1 if recover() found open transactions
);

CREATE TABLE IF NOT EXISTS transactions (
  tx_id       INTEGER PRIMARY KEY,
  kind        TEXT NOT NULL CHECK (kind IN ('session','persistent','global_reset','foreign_restore')),
  state       TEXT NOT NULL CHECK (state IN ('open','applying','applied','reverting','reverted','failed','abandoned')),
  game_id     TEXT,                      -- e.g. 'steam:730' (session / persistent per game)
  game_pid    INTEGER,
  game_start  INTEGER,                   -- process start time: (pid, start) identifies the instance
  boot_id     TEXT NOT NULL REFERENCES boots(boot_id),
  reason      TEXT,                      -- 'game_start', 'user_accept', 'drift_repair', 'recovery', ...
  created_at  INTEGER NOT NULL,
  closed_at   INTEGER,
  error_code  TEXT
);
CREATE INDEX IF NOT EXISTS ix_tx_open ON transactions(state) WHERE state IN ('open','applying','applied','reverting');

CREATE TABLE IF NOT EXISTS steps (
  step_id        INTEGER PRIMARY KEY,
  tx_id          INTEGER NOT NULL REFERENCES transactions(tx_id),
  seq            INTEGER NOT NULL,       -- apply order; revert runs in reverse
  tweak_id       TEXT NOT NULL,
  tweak_version  INTEGER NOT NULL,
  action_kind    TEXT NOT NULL CHECK (action_kind IN (
                   'registry_value','service_start','power_scheme','power_overlay','scheduled_task',
                   'ifeo_perf_options','gpu_preference','driver_setting','file_patch','process_priority',
                   'process_eco_qos','process_suspend')),
  target_json    TEXT NOT NULL,          -- what (key/name, service, exe, file, ProcId...)
  before_json    TEXT,                   -- captured original state; {"absent":true} when it did not exist
  after_json     TEXT,                   -- intended value
  state          TEXT NOT NULL CHECK (state IN (
                   'planned','captured','applied','verified','apply_failed',
                   'reverted','revert_verified','revert_failed','superseded')),
  attempts       INTEGER NOT NULL DEFAULT 0,
  error_code     TEXT,
  error_detail   TEXT,
  updated_at     INTEGER NOT NULL,
  UNIQUE (tx_id, seq)
);
CREATE INDEX IF NOT EXISTS ix_steps_tx ON steps(tx_id, seq);
-- Persistent changes currently alive (used by Drift Guardian and global reset)
CREATE INDEX IF NOT EXISTS ix_steps_alive ON steps(tweak_id) WHERE state IN ('applied','verified');

CREATE TABLE IF NOT EXISTS file_backups (
  sha256      TEXT PRIMARY KEY,
  original_path TEXT NOT NULL,
  stored_path TEXT NOT NULL,             -- copy under %ProgramData%\Overdrive\backups\
  size_bytes  INTEGER NOT NULL,
  created_at  INTEGER NOT NULL
);

-- Measured sessions (Module B/E)
CREATE TABLE IF NOT EXISTS sessions (
  session_id    INTEGER PRIMARY KEY,
  tx_id         INTEGER REFERENCES transactions(tx_id),
  game_id       TEXT NOT NULL,
  is_baseline   INTEGER NOT NULL CHECK (is_baseline IN (0,1)),
  tweak_set     TEXT NOT NULL,           -- JSON array of active tweak ids (sorted)
  context_json  TEXT NOT NULL,           -- resolution, settings hash, driver version, power source, hw fingerprint
  started_at    INTEGER NOT NULL,
  gameplay_secs REAL NOT NULL,
  avg_fps REAL, low_1 REAL, low_01 REAL, p50_ms REAL, p95_ms REAL, p99_ms REAL, stdev_ms REAL,
  stutters INTEGER,
  bg_wakeups_per_s REAL,                 -- for BackgroundNoise-measured tweaks
  bottleneck    TEXT,
  valid         INTEGER NOT NULL DEFAULT 1 -- 0 = excluded from A/B (too short, context mismatch)
);
CREATE INDEX IF NOT EXISTS ix_sessions_game ON sessions(game_id, started_at);

-- A/B verdicts per machine (Module E) — the machine's learned "truth"
CREATE TABLE IF NOT EXISTS tweak_outcomes (
  tweak_id    TEXT NOT NULL,
  game_id     TEXT NOT NULL,             -- '*' for system-wide tweaks
  verdict     TEXT NOT NULL CHECK (verdict IN ('kept','no_benefit','harmful','inconclusive')),
  delta_low1_pct REAL,
  ci_low REAL, ci_high REAL,             -- 95% confidence interval of the delta
  runs        INTEGER NOT NULL,
  decided_at  INTEGER NOT NULL,
  PRIMARY KEY (tweak_id, game_id)
);

-- Background isolation (EcoQoS / priority / suspend) is journaled as ordinary steps
-- (action_kind process_*), so it gets the same write-ahead + recovery guarantees.

-- Drift Guardian (Module I)
CREATE TABLE IF NOT EXISTS drift_events (
  id          INTEGER PRIMARY KEY,
  trigger     TEXT NOT NULL,             -- windows_update | driver_installed | boot | daily
  finding     TEXT NOT NULL,             -- reverted | no_longer_beneficial | regression | health
  step_id     INTEGER REFERENCES steps(step_id),
  detail_json TEXT,
  resolution  TEXT CHECK (resolution IN ('auto_repaired','asked_user','user_declined','ignored') OR resolution IS NULL),
  created_at  INTEGER NOT NULL
);

-- BIOS Coach (§5.3b)
CREATE TABLE IF NOT EXISTS coach_flows (
  id          INTEGER PRIMARY KEY,
  flow_id     TEXT NOT NULL,             -- e.g. 'ram_xmp'
  state       TEXT NOT NULL CHECK (state IN ('started','rebooted_to_fw','verified_applied','not_applied','unstable','rolled_back','done')),
  before_json TEXT NOT NULL,             -- e.g. configured RAM speed before
  after_json  TEXT,
  watch_until INTEGER,                   -- stability watch end (unix ms)
  updated_at  INTEGER NOT NULL
);
