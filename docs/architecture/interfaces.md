# Interfaces de los módulos

Firmas de diseño en Rust. Se convierten en código compilable al crear el workspace (Fase 3). Los nombres siguen `goals.md §4.4`.

**Principios aplicados a todos los módulos**
- Errores tipados (`thiserror`), nunca `String` sueltos. Cada error tiene un `user_message_key` de i18n con un texto accionable.
- Toda operación que toca el sistema recibe un `Deadline`, para que nada pueda colgarse indefinidamente (lección v0.1.1).
- Todo dato inferido lleva su `Confidence`.
- El núcleo (`overdrive-core`) solo depende de los **puertos** de §0. Las implementaciones reales viven en `overdrive-win` y las de test en `overdrive-core::testing`.

---

## 0. Puertos del sistema (`SystemPort`)

Capa de abstracción que hace testeable todo lo demás.

```rust
pub struct Deadline(pub std::time::Instant);

pub trait Registry: Send + Sync {
    fn read(&self, key: &RegPath, name: &str) -> Result<Option<RegValue>, SysError>; // None = no existe
    fn write(&self, key: &RegPath, name: &str, value: &RegValue) -> Result<(), SysError>;
    fn delete_value(&self, key: &RegPath, name: &str) -> Result<(), SysError>;
    fn key_exists(&self, key: &RegPath) -> Result<bool, SysError>;
    fn create_key(&self, key: &RegPath) -> Result<(), SysError>;
    fn delete_key_if_empty(&self, key: &RegPath) -> Result<(), SysError>;
}

pub trait Services: Send + Sync {
    fn query(&self, name: &str) -> Result<Option<ServiceState>, SysError>; // start_type + running
    fn set_start_type(&self, name: &str, t: StartType) -> Result<(), SysError>;
    fn start(&self, name: &str, d: Deadline) -> Result<(), SysError>;
    fn stop(&self, name: &str, d: Deadline) -> Result<(), SysError>;
}

pub trait Power: Send + Sync {
    fn active_scheme(&self) -> Result<Guid, SysError>;
    fn set_active_scheme(&self, g: &Guid) -> Result<(), SysError>;
    fn overlay_mode(&self) -> Result<Option<Guid>, SysError>;      // "modo de energía" de Win10/11
    fn set_overlay_mode(&self, g: &Guid) -> Result<(), SysError>;
    fn power_source(&self) -> Result<PowerSource, SysError>;       // Ac | Battery
}

pub trait Files: Send + Sync {
    fn read(&self, p: &Path) -> Result<Option<Vec<u8>>, SysError>;
    fn write_atomic(&self, p: &Path, bytes: &[u8]) -> Result<(), SysError>; // temp + fsync + rename
    fn hash(&self, p: &Path) -> Result<Option<Sha256>, SysError>;
}

pub trait Processes: Send + Sync {
    fn list(&self) -> Result<Vec<ProcInfo>, SysError>;              // pid, image, start_time, session, parent
    fn priority(&self, id: ProcId) -> Result<PriorityClass, SysError>;
    fn set_priority(&self, id: ProcId, p: PriorityClass) -> Result<(), SysError>;
    fn set_eco_qos(&self, id: ProcId, on: bool) -> Result<(), SysError>;
    fn suspend(&self, id: ProcId) -> Result<(), SysError>;
    fn resume(&self, id: ProcId) -> Result<(), SysError>;
    fn is_producing_audio(&self, id: ProcId) -> Result<bool, SysError>; // WASAPI session enumeration
    fn foreground(&self) -> Result<Option<ProcId>, SysError>;
}
/// `ProcId` = (pid, start_time): un PID reutilizado nunca se confunde con el original.
pub struct ProcId { pub pid: u32, pub start_time: u64 }

pub trait ScheduledTasks: Send + Sync {
    fn enabled(&self, path: &str) -> Result<Option<bool>, SysError>;
    fn set_enabled(&self, path: &str, on: bool) -> Result<(), SysError>;
}

pub trait DriverProfiles: Send + Sync {              // NVAPI DRS / ADLX
    fn vendor(&self) -> GpuVendor;
    fn get(&self, exe: &str, setting: DriverSetting) -> Result<Option<DriverValue>, SysError>;
    fn set(&self, exe: &str, setting: DriverSetting, v: &DriverValue) -> Result<(), SysError>;
    fn clear(&self, exe: &str, setting: DriverSetting) -> Result<(), SysError>; // volver al valor heredado
}

pub trait Clock: Send + Sync { fn now(&self) -> Timestamp; }

pub struct SystemPort {
    pub registry: Arc<dyn Registry>, pub services: Arc<dyn Services>, pub power: Arc<dyn Power>,
    pub files: Arc<dyn Files>, pub processes: Arc<dyn Processes>, pub tasks: Arc<dyn ScheduledTasks>,
    pub drivers: Arc<dyn DriverProfiles>, pub clock: Arc<dyn Clock>,
}
```

---

## A — Hardware & Environment Profiler

```rust
pub enum Confidence { Exact, Inferred, Unknown }
pub struct Field<T> { pub value: Option<T>, pub confidence: Confidence, pub source: &'static str }

pub struct HardwareProfile {
    pub schema_version: u32,
    pub cpu: CpuInfo,            // topología P/E, CCD, relojes, Field<bool> throttling
    pub memory: MemoryInfo,      // capacidad, configured vs rated speed, channels: Field<u8>
    pub gpus: Vec<GpuInfo>,      // dedicada/integrada, VRAM, LUID, driver, PCIe actual/máx, ReBAR
    pub storage: Vec<DriveInfo>, // tipo, % libre, salud
    pub power: PowerContext,     // portátil, AC/batería, plan
    pub windows: WindowsGamingSettings, // Game Mode, HAGS, DVR, VRR, MPO, reinicio pendiente…
    pub firmware: FirmwareInfo,  // placa, BIOS, versión
    pub tier: Tier,              // Tier1 | Tier2 | Tier3
}

pub struct StructuralIssue {
    pub id: IssueId,                  // p.ej. RamBelowRatedSpeed, SingleChannelRam, SsdNearlyFull
    pub impact: Impact,               // High | Medium | Low (orden de la lista)
    pub confidence: Confidence,
    pub fixable_by: FixPath,          // Software(TweakId) | BiosCoach(CoachFlowId) | Hardware | UserAction
    pub explanation_key: I18nKey,
}

pub trait HardwareProfiler {
    fn profile(&self, d: Deadline) -> Result<HardwareProfile, ProfilerError>;
    fn structural_issues(&self, p: &HardwareProfile) -> Vec<StructuralIssue>;
    fn classify_tier(&self, p: &HardwareProfile) -> Tier;
}

/// Fuentes de datos que implementa overdrive-win (WMI/CIM, DXGI, SetupAPI, PDH, SMBIOS).
pub trait HardwareSource {
    fn cpu(&self) -> Result<CpuInfo, SysError>;
    fn memory_modules(&self) -> Result<Vec<MemoryModule>, SysError>;
    fn adapters(&self) -> Result<Vec<GpuInfo>, SysError>;
    fn drives(&self) -> Result<Vec<DriveInfo>, SysError>;
    fn perf_limit_sample(&self, window: Duration) -> Result<PerfLimitSample, SysError>;
    // …
}
```

### BIOS Coach (§5.3b)

```rust
pub struct CoachFlow { pub id: CoachFlowId, pub steps: Vec<CoachStep>, pub vendor: BoardVendor }
pub enum CoachStep { Explain(I18nKey), RebootToFirmware, BiosInstruction { menu_path: Vec<I18nKey>, setting: String, recommended: String } }

pub trait BiosCoach {
    fn flow_for(&self, issue: &StructuralIssue, hw: &HardwareProfile) -> Option<CoachFlow>;
    fn reboot_to_firmware(&self) -> Result<(), SysError>;              // shutdown /r /fw equivalente
    fn verify_after_reboot(&self, flow: &CoachFlowId, hw_now: &HardwareProfile) -> CoachVerdict; // Applied | NotApplied
    fn stability_watch(&self, since: Timestamp) -> StabilityReport;   // WHEA, BSOD, apagados inesperados
}
```

---

## B — Telemetry & Bottleneck Classifier

```rust
pub struct FrameSample { pub t: Timestamp, pub frametime_ms: f32, pub gpu_busy_ms: Option<f32>, pub display_latency_ms: Option<f32> }
pub struct SystemSample { pub t: Timestamp, pub ram_commit_pct: f32, pub vram_usage_pct: Option<f32>,
                          pub hard_faults_per_s: f32, pub disk_queue: f32, pub perf_limit_pct: f32, pub on_battery: bool }

pub trait TelemetrySource {
    fn start(&mut self, game: ProcId) -> Result<CaptureHandle, TelemetryError>;
    fn poll(&mut self, h: &CaptureHandle) -> Vec<FrameSample>;   // no bloqueante
    fn stop(&mut self, h: CaptureHandle) -> Result<(), TelemetryError>;
}

pub struct SessionMetrics { pub avg_fps: f32, pub low_1: f32, pub low_01: f32,
                            pub p50_ms: f32, pub p95_ms: f32, pub p99_ms: f32,
                            pub stdev_ms: f32, pub stutters: u32, pub gameplay_secs: f32 }

pub enum Bottleneck { GpuBound, CpuBound, MemoryBound, IoBound, ThermalOrPowerLimited, Mixed, Unknown }
pub struct Classification { pub label: Bottleneck, pub confidence: f32, pub evidence: Vec<EvidenceItem> }

pub trait MetricsCalculator { fn compute(&self, frames: &[FrameSample], exclude: &[TimeRange]) -> SessionMetrics; }

pub trait BottleneckClassifier {
    /// Ventana deslizante con histéresis; `thresholds` son constantes configurables documentadas.
    fn classify(&self, frames: &[FrameSample], sys: &[SystemSample], th: &Thresholds) -> Classification;
    fn non_gameplay_ranges(&self, frames: &[FrameSample], sys: &[SystemSample]) -> Vec<TimeRange>; // menús/cargas
}
```

---

## C — Optimization Catalog

Los tweaks son **datos** (TOML en `catalog/`). El código solo implementa un conjunto cerrado de **primitivas**, y cada tweak es una lista de ellas.

```rust
pub struct TweakDef {
    pub id: TweakId, pub version: u32,
    pub title: I18nKey, pub explanation: I18nKey, pub may_break: Option<I18nKey>,
    pub applies_when: Predicate,               // tiers, bottlenecks, versión de Windows, predicados de hardware
    pub scope: Scope,                          // Session | Persistent
    pub requires_reboot: bool,
    pub risk: Risk,                            // Low | Medium | High
    pub evidence: Evidence,                    // Proven | VersionDependent | Disputed (+ notas/fuentes)
    pub anticheat_safe: bool,
    pub measure: MeasureKind,                  // Frametime | BackgroundNoise | None(informativo)
    pub actions: Vec<Action>,
}

/// Conjunto cerrado de primitivas: es lo único que el motor sabe hacer.
pub enum Action {
    RegistryValue   { key: RegPath, name: String, value: RegValue },
    ServiceStart    { name: String, start: StartType, stop_now: bool },
    PowerScheme     { scheme: Guid },
    PowerOverlay    { mode: Guid },
    ScheduledTask   { path: String, enabled: bool },
    IfeoPerfOptions { exe: String, cpu_priority: Option<u32>, io_priority: Option<u32> },
    GpuPreference   { exe_path: PathBuf, pref: GpuPref },   // UserGpuPreferences
    DriverSetting   { exe: String, setting: DriverSetting, value: DriverValue },
    FilePatch       { game: GameId, file: PathBuf, patch: IniPatch },   // Fase 7
    BackgroundIsolation,                                     // delega en Safety Policy en runtime
}

/// Cada primitiva sabe capturar, aplicar, verificar y revertir. Todas idempotentes.
pub trait Primitive {
    fn capture(&self, sys: &SystemPort) -> Result<Snapshot, SysError>;            // estado original (incluye "no existía")
    fn apply(&self, sys: &SystemPort, d: Deadline) -> Result<(), SysError>;
    fn verify_applied(&self, sys: &SystemPort) -> Result<bool, SysError>;
    fn revert(&self, sys: &SystemPort, before: &Snapshot, d: Deadline) -> Result<(), SysError>;
    fn verify_reverted(&self, sys: &SystemPort, before: &Snapshot) -> Result<bool, SysError>;
}

pub trait Catalog {
    fn all(&self) -> &[TweakDef];
    fn get(&self, id: &TweakId) -> Option<&TweakDef>;
    /// Filtra por hardware, bottleneck, modo (Fácil = Low + Proven), anti-cheat y lista negra de la máquina.
    fn candidates(&self, ctx: &PlanContext) -> Vec<PlannedTweak>;
    /// Detecta tweaks dañinos aplicados por otras herramientas (lista "fuera de alcance").
    fn foreign_harmful(&self, sys: &SystemPort) -> Vec<ForeignTweak>;
}
```

---

## D — Transaction Engine

Detalle en [transaction-engine.md](transaction-engine.md).

```rust
pub trait Journal {                                   // SQLite WAL; ver journal.sql
    fn begin(&self, kind: TxKind, meta: TxMeta) -> Result<TxId, JournalError>;
    fn record_before(&self, tx: TxId, step: &StepRecord) -> Result<StepId, JournalError>; // COMMIT antes de aplicar
    fn mark(&self, step: StepId, state: StepState, err: Option<&StepError>) -> Result<(), JournalError>;
    fn close(&self, tx: TxId, state: TxState) -> Result<(), JournalError>;
    fn open_transactions(&self) -> Result<Vec<TxRecord>, JournalError>;
    fn persistent_steps_alive(&self) -> Result<Vec<StepRecord>, JournalError>; // para Drift Guardian y reset global
}

pub trait TransactionEngine {
    fn apply(&self, kind: TxKind, plan: &[PlannedTweak], d: Deadline, progress: &dyn ProgressSink) -> Result<TxOutcome, TxError>;
    fn revert(&self, tx: TxId, d: Deadline, progress: &dyn ProgressSink) -> Result<TxOutcome, TxError>;
    fn revert_tweak(&self, tweak: &TweakId, d: Deadline) -> Result<TxOutcome, TxError>; // deshacer un persistente concreto
    fn revert_all(&self, d: Deadline, progress: &dyn ProgressSink) -> Result<ResetReport, TxError>; // reset global verificado
    fn recover(&self, d: Deadline) -> Result<RecoveryReport, TxError>;  // al arrancar, ANTES de aceptar IPC
}

pub trait ProgressSink { fn step(&self, index: u32, total: u32, tweak: &TweakId, state: StepState); }
```

---

## E — Session Orchestrator

```rust
pub enum GameEvent { Started { game: GameId, proc: ProcId }, Stopped { game: GameId, proc: ProcId } }

pub trait GameDetector {               // ETW Microsoft-Windows-Kernel-Process: sin sondeo, sin handles al juego
    fn subscribe(&self, tx: Sender<GameEvent>) -> Result<(), SysError>;
    fn library(&self) -> Result<Vec<GameEntry>, SysError>;   // Steam, Epic, EA, Ubisoft, Battle.net, GOG, Xbox
}

pub enum TweakVerdict { Kept, NoMeasurableBenefit, Harmful, Inconclusive { runs_needed: u32 } }

pub trait VerificationPolicy {
    /// A/B: mejora del 1 % low o de la desviación del frametime por encima del ruido medido (IC 95 %).
    fn verdict(&self, baseline: &[SessionMetrics], candidate: &[SessionMetrics]) -> (TweakVerdict, ConfidenceInterval);
    fn should_offer_persistent(&self, history: &TweakHistory) -> bool;
}

pub trait SessionOrchestrator {
    fn on_event(&mut self, ev: GameEvent) -> Result<(), SessionError>;
    fn active_session(&self) -> Option<SessionView>;
    fn run_guided_benchmark(&mut self, game: GameId) -> Result<OpId, SessionError>;
}
```

---

## F — Safety Policy

```rust
pub enum AntiCheat { None, Kernel(AcKind), UserMode(AcKind), Unknown }  // Vanguard, EAC, BattlEye, Ricochet, FACEIT, Javelin, GameGuard, XIGNCODE…

pub enum BackgroundDecision {
    NeverTouch(Reason),    // audio activo, voz en llamada, launcher/DRM, anti-cheat, seguridad, accesibilidad,
                           // periféricos, sistema/shell, primer plano, lista blanca del usuario
    LowerPriority,         // por defecto: EcoQoS + BelowNormal
    Suspend,               // solo categorías seguras conocidas (updaters, navegador sin reproducción) y visibles al usuario
}

pub trait SafetyPolicy {
    fn anticheat_status(&self, game: &GameEntry) -> AntiCheat;
    /// Con anti-cheat de kernel: nunca abrir handles al proceso del juego ni tocar sus ficheros.
    fn filter_plan(&self, plan: Vec<PlannedTweak>, game: &GameEntry, mode: UxMode) -> FilteredPlan;
    fn background_action(&self, p: &ProcInfo, ctx: &BackgroundContext) -> BackgroundDecision;
    /// Se invoca en: fin de juego, parada del servicio, recover(), "Reanudar todo",
    /// y cuando un proceso suspendido pasa a primer plano.
    fn resume_all(&self, reason: ResumeReason) -> ResumeReport;
}
```

---

## G — UI & Reporting

La UI no implementa lógica: consume un **snapshot** y aplica parches (ver [ipc-protocol.md](ipc-protocol.md)).

```rust
pub struct EngineSnapshot {
    pub seq: u64,
    pub hardware: Option<HardwareSummary>, pub issues: Vec<StructuralIssue>,
    pub pulse: PulseState,                  // Idle | Bottleneck(label, intensity) | Thermal(pressure) | SessionActive
    pub games: Vec<GameCard>, pub active_session: Option<SessionView>,
    pub operations: Vec<OperationView>,     // TODAS las ops en curso: la UI solo muestra overlays de las que están aquí
    pub persistent_changes: Vec<PersistentChangeView>, pub suspended: Vec<SuspendedView>,
    pub mode: UxMode,                       // Easy | Expert
}

pub trait Reporter {
    fn session_report(&self, tx: TxId) -> Result<SessionReport, ReportError>;  // baseline vs optimizado, kept/reverted y por qué
    fn monthly_health(&self, month: YearMonth) -> Result<HealthSummary, ReportError>;
}
```

---

## H — Community & Sharing (en `overdrive-ui`, opt-in)

```rust
pub trait Sharing {
    fn frametime_card(&self, report: &SessionReport) -> Result<CardImage, ShareError>;   // solo sesiones medidas
    fn sign_card(&self, card: &CardImage, report: &SessionReport) -> CardHash;
    fn upload_preview(&self, payload: &CommunityPayload) -> RedactedPreview;             // "qué enviamos"
    fn help_a_friend_export(&self) -> Result<FriendCode, ShareError>;                    // diagnóstico redactado
    fn help_a_friend_import(&self, code: &FriendCode) -> Result<FriendDiagnostic, ShareError>;
    fn recommend_profile(&self, diag: &FriendDiagnostic, profile: &Profile) -> Result<FriendCode, ShareError>;
}
```

---

## I — Drift Guardian

```rust
pub enum DriftTrigger { WindowsUpdateInstalled, DriverInstalled, Boot, Daily }
pub enum DriftFinding {
    Reverted { step: StepId, tweak: TweakId },         // el cambio persistente ya no está
    NoLongerBeneficial { tweak: TweakId },             // sigue aplicado pero ya no aporta
    PerformanceRegression { game: GameId, cause: Option<RegressionCause> }, // driver nuevo, app de fondo, temperatura, disco
    HealthSignal(HealthSignal),                        // disco lleno, apps de inicio nuevas, throttling creciente
}

pub trait DriftGuardian {
    fn check(&self, trigger: DriftTrigger, d: Deadline) -> Result<Vec<DriftFinding>, DriftError>;
    /// Reaplica solo si el riesgo es Low y ya fue aprobado; si no, genera una pregunta en lenguaje sencillo.
    fn repair(&self, f: &DriftFinding, d: Deadline) -> Result<RepairOutcome, DriftError>;
}
```
