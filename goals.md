# Overdrive — Master Product & Architecture Brief

> How to use this file: **Part I** is the master brief (shared context for every task).
> **Part II** contains one self-contained prompt per phase. Always send Part I + exactly one phase prompt.
> Never ask for all phases at once.

---

# PART I — MASTER BRIEF

## 0. Role

Act as a Principal Software Architect, Senior Windows Systems Engineer, Performance Engineer and Lead UI/UX Product Designer. Prefer measured evidence over folklore, and say explicitly when a technique is unproven, version-dependent or risky.

## 1. Context: what already exists (Overdrive v0.2.1)

Overdrive is an existing, working Windows game optimizer. It is **not** a greenfield project; reuse its knowledge and avoid its past mistakes.

- **Current stack:** Electron + Node.js, with system work done via PowerShell (`src/main/ps.js`). Packaged with electron-builder (NSIS + portable), auto-update via GitHub Releases (`updater.js`).
- **Current modules:** `system.js` (hardware/system analysis), `tweaks.js` (reversible tweak plan), `games.js` (game library detection), `profiles.js` (per-game profiles), `benchmark.js`, `monitor.js` (real-time monitor), `suspend.js` (background process suspension), renderer UI with **Easy mode / Expert mode**.
- **Lessons learned from past bugs (must not regress):**
  - v0.2.1 — "Game Mode" **froze user apps** instead of freeing RAM (unsafe process suspension). → Suspension needs a strict safety policy (§5.4).
  - v0.1.2 — a blocking "Applying…" overlay appeared on every launch (UI state not tied to real engine state). → UI must render from engine state, never from assumptions.
  - v0.1.1 — restore-point creation hung the flow; cancel semantics were confusing. → Every system call needs a timeout, progress reporting and a defined failure path.
  - Admin elevation broke in the packaged build. → Privilege model must be designed up front (§4.2), not patched later.

## 1b. Vision & target user

**Why this exists:** the author optimized their own PC by trial and error. Many popular tweaks did nothing, some helped, and some made performance *worse*. Overdrive turns that painful experience into a tool that does it right, measurably, for people who can't.

**Primary user — "the friend who knows nothing":**
- Doesn't know what RAM channels, drivers, VRAM or a power plan are, and shouldn't need to.
- Won't read long explanations, won't tune sliders, won't diagnose errors.
- Needs to *trust* the app: it must never break their PC, and it must show proof that it helped.
- Success = they press one button, play, and the game is noticeably smoother **weeks later**, not just today.

**Secondary user — the author / the "tech friend":** wants full transparency (Expert mode) and the ability to review a friend's diagnostics remotely to help them.

**Differentiation (the market gap):** existing optimizers either apply blind, generic tweak packs (often placebo or harmful) or expose overwhelming expert panels. Overdrive is the only one that **(1) measures on your machine, (2) keeps only what proved useful, (3) keeps it working over time, and (4) explains it in plain language** — wrapped in a precise, beautiful interface.

## 2. Product philosophy

1. **Zero snake oil, provably.** No tweak is "good" by default. Every optimization is (a) mapped to a diagnosed bottleneck, (b) measured on *this* machine with a before/after protocol, and (c) kept only if it improves results beyond measurement noise; otherwise it is reverted automatically and reported as "no measurable benefit".
2. **Never promise before measuring.** The UI may show *what* will be tried and *why*, but never a numeric gain until it has been measured locally (or comes from a verified community dataset for identical hardware, clearly labeled as such).
3. **One-intent UX.** The user expresses intent ("make this game smoother"); the engine handles Standby Lists, priorities and driver profiles. Technical depth is available, never required.
4. **Safe by construction.** Every change is journaled before it happens, reversible, and verified after reversal. The OS must end up exactly as it was, even after a crash, a kill or a power loss.
5. **Respect the platform.** No kernel drivers, no DLL injection, no memory writes into games, no touching anti-cheat-protected processes.
6. **Light footprint.** An optimizer for weak PCs must not be a weak PC's problem.
7. **Lasting value over one-shot boosts.** The goal is a PC that *stays* optimized. Changes that proved useful are made persistent (with consent), and the app actively guards them: Windows updates, driver updates and new software silently undo or degrade settings, and Overdrive detects and repairs that drift (§5.8).
8. **Zero knowledge required.** The default experience must be completable by someone who has never opened Task Manager: one action, plain-language results, no jargon without a one-line explanation, safe defaults everywhere.
9. **Honesty as a feature.** The app openly says "this tweak did nothing on your PC, so we undid it" and "this one made it worse". Showing what *didn't* work builds the trust competitors lack.

## 3. Target users, platforms and constraints

- **Hardware:** low-to-mid-end PCs and laptops. Tier classification:
  - **Tier 1 — Ultra-low:** iGPU only, ≤ 8 GB RAM, ≤ 4 cores, often HDD or near-full SSD.
  - **Tier 2 — Entry dedicated:** dedicated GPU ≤ 4 GB VRAM, 8 GB RAM, 4–6 cores.
  - **Tier 3 — Mid-range bottlenecked:** decent parts held back by one weak component (single-channel RAM, XMP off, PCIe x4/x8 GPU on Gen3, thermal throttling, 85 %+ full SSD).
  - Laptops are first-class citizens (hybrid GPUs, battery, thermals).
- **OS:** Windows 10 22H2 and Windows 11 23H2+. Document every behavior that differs between them.
- **Language:** UI in Spanish first, fully i18n-ready (ES/EN at launch). Code, comments and logs in English.
- **Distribution:** signed installer + portable build, silent auto-update with signed manifests.

## 4. Architecture requirements

### 4.1 Stack
- **Target:** Rust core + Tauri 2 (WebView2) UI. Justify against .NET 8 Native AOT + WinUI 3 and against keeping Electron. The decision must consider migration cost from the existing Electron/PowerShell code.
- **No PowerShell in hot paths.** Use native Win32/NT APIs, WMI/CIM via COM, SetupAPI, PDH, ETW. PowerShell is allowed only as a documented fallback.

### 4.2 Process model (mandatory split)
1. **`overdrive-svc` — privileged Windows Service (LocalSystem).** Owns the optimization engine, the transaction journal, rollback, game launch/exit detection and telemetry capture. Exposes a minimal, versioned, authenticated IPC (named pipe with ACL restricted to the interactive user + request schema validation). No arbitrary command execution over IPC — only whitelisted, typed operations.
2. **`overdrive-ui` — unprivileged UI.** Never needs UAC after installation. Can be closed at any time without affecting a running session.
3. **Watchdog / recovery.** If the service crashes or the machine loses power mid-session, the next service start detects the dirty journal and rolls back before doing anything else.

### 4.3 Footprint budgets (measured, part of CI)
- Service idle (no game running): **< 15 MB private working set, < 0.1 % CPU** averaged over 10 min.
- Service during a game session with telemetry: **< 30 MB, < 0.5 % CPU** of one core.
- UI: destroyed (not hidden) when minimized to tray; WebView2 memory is not counted toward the idle budget but must stay < 150 MB while open.
- Event-driven, not polling: process start/stop via ETW (`Microsoft-Windows-Kernel-Process`), not WMI polling loops.

### 4.4 Module boundaries
- **A. Hardware & Environment Profiler**
- **B. Telemetry & Bottleneck Classifier**
- **C. Optimization Catalog** (declarative tweak definitions)
- **D. Transaction Engine** (journal, apply, verify, rollback)
- **E. Session Orchestrator** (game lifecycle → apply/measure/revert)
- **F. Safety Policy** (anti-cheat, suspension safelist, risk levels)
- **G. UI & Reporting**
- **H. Community & Sharing** (opt-in)
- **I. Drift Guardian & Long-term Health** (keeps proven changes working over time)

Each module must be testable in isolation with a fake/mock provider (fake hardware, recorded telemetry traces, in-memory registry).

## 5. Functional specification

### 5.1 Module A — Hardware & Environment Profiler
Runs once at install and incrementally on hardware/driver changes. For every field, state the data source and a confidence level (`exact | inferred | unknown`); never present inferred data as fact.

- **CPU:** vendor, model, cores/threads, P-core/E-core layout (`GetLogicalProcessorInformationEx`), CCD/CCX layout, base/boost clocks.
  - **Throttling detection without drivers:** PDH counters `Processor Information\% Performance Limit` and `% of Maximum Frequency` under load. **Do not ship or load any kernel driver** (e.g. WinRing0 is flagged as a vulnerable driver by Microsoft Defender).
- **Memory:** total capacity, configured vs. rated speed (SMBIOS `ConfiguredClockSpeed` vs `Speed` → flag "XMP/EXPO likely disabled"), module count and slot locators → single/dual-channel **inference** (labeled `inferred`), iGPU shared memory.
- **GPU & bus:** all adapters via DXGI (dedicated vs integrated, VRAM, LUID), driver version/date, DX12 Ultimate / Vulkan 1.3 / ReBAR support, current vs max PCIe link width and generation (SetupAPI `DEVPKEY_PciDevice_CurrentLinkWidth/Speed`).
  - **Hybrid-GPU laptops:** detect Optimus/AMD switchable setups and which adapter each game actually renders on.
- **Storage:** type (HDD / SATA SSD / NVMe), free-space %, (warn SSD > 85 % full), SMART/NVMe health where readable without drivers, which drive each game lives on.
- **Power & thermals context:** AC vs battery, active power plan / power mode, laptop vs desktop, lid/cooling hints.
- **Windows gaming settings:** Game Mode, HAGS, "Optimizations for windowed games", Game Bar background recording (Game DVR), VRR, MPO status, pending Windows Update reboots, Defender scheduled scans.
- **Output:** a versioned `HardwareProfile` JSON + Tier + a ranked list of *structural* issues the software cannot fix but the user should know (e.g. "RAM running single-channel: adding a second stick is the single biggest upgrade for this PC").

### 5.2 Module B — Telemetry & Bottleneck Classifier
- **Capture:** PresentMon 2.x (ETW-based, no injection). Metrics: avg FPS, 1 % low, 0.1 % low, frametime p50/p95/p99, frametime standard deviation, stutter count (frames > 2× rolling median), **GPU Busy** and display latency where available.
- **Classification (primary signal = GPU Busy vs frametime, not utilization %):**
  - `GPU_BOUND`: GPU Busy ≈ frametime (within ~10 %) for the majority of frames.
  - `CPU_BOUND`: GPU Busy substantially below frametime, GPU idling between frames.
  - `MEMORY_BOUND`: RAM commit > 90 % or VRAM usage > 90 % of budget (DXGI `QueryVideoMemoryInfo`) *with* elevated hard faults/sec correlated with frametime spikes.
  - `IO_BOUND`: frametime spikes correlated with disk queue length / read latency on the game's drive.
  - `THERMAL/POWER_LIMITED`: % Performance Limit rising over the session, or battery power.
  - `MIXED/UNKNOWN`: allowed and honest; never force a label.
- Thresholds are **configurable constants with documented rationale**, and the classifier must be validated against a set of recorded traces (unit tests).
- Classification uses a sliding window and hysteresis; menus/loading screens are excluded (detect via low GPU load + static frames or user markers).

### 5.3 Module C — Optimization Catalog
Each tweak is a **declarative entry**, not ad-hoc code:

```
id, title_i18n, explanation_i18n (plain language "why"),
applies_when: { tiers, bottlenecks, os_versions, hardware predicates },
scope: SESSION | PERSISTENT,          // SESSION = applied on game launch, reverted on exit
requires_reboot: bool,                // PERSISTENT + reboot ⇒ explicit user consent
risk: LOW | MEDIUM | HIGH,
anticheat_safe: bool,
evidence: { status: PROVEN | VERSION_DEPENDENT | DISPUTED, sources/notes },
apply(), verify_applied(), revert(), verify_reverted()   // all idempotent
```

**Priority order (highest real-world impact on low-end first):**
1. **Hybrid GPU fix:** force the high-performance GPU per executable (`HKCU\Software\Microsoft\DirectX\UserGpuPreferences`). Persistent, low risk.
2. **Smart frame cap:** cap slightly below the sustainable 1 % low to stabilize frametimes when CPU-bound (NVIDIA per-app Max Frame Rate via NVAPI DRS; AMD via ADLX where supported; otherwise recommend in-game cap). Suggest Reflex/Anti-Lag when the game supports it.
3. **Windows gaming settings:** Game Mode on, HAGS (only where beneficial for that GPU/driver), windowed-game optimizations, disable Game Bar background recording, pause Windows Update / defer Defender scheduled scans during sessions.
4. **Power:** high-performance power mode during sessions on AC; on battery, warn instead of tweaking.
5. **Game process priority without touching the process:** `Image File Execution Options\<exe>\PerfOptions` (`CpuPriorityClass`, `IoPriority`) — avoids opening handles to anti-cheat-protected processes.
6. **Smart background isolation:** suspend/de-prioritize non-essential background apps (see §5.4 policy).
6b. **Background noise reduction — telemetry & bloat (PERSISTENT, proven useful by the author):** reduce periodic background CPU/disk spikes that cause stutter on weak PCs. Curated, reversible list only, e.g.:
   - Windows: `DiagTrack` (Connected User Experiences and Telemetry) → manual/disabled, telemetry level to minimum allowed by the edition, CEIP / Compatibility Appraiser / Program Data Updater scheduled tasks disabled, advertising ID and "tailored experiences" off.
   - Third-party: NVIDIA/AMD telemetry containers & tasks, OEM telemetry/updaters (HP, Lenovo, Acer, ASUS, MSI…), Office telemetry tasks, unnecessary startup entries (shown to the user, never removed silently).
   - **Never:** hosts-file/firewall blocking of Microsoft endpoints, removing Windows Update, Store, Defender, or components other features depend on. Each entry documents what it might break.
   - **Measured differently:** benefit is verified via background CPU wakeups, disk activity and stutter count during idle and gameplay, not only average FPS.
7. **Driver profile (per executable):** NVAPI DRS (power management "prefer maximum performance", texture filtering quality, low-latency mode — disabled if severely CPU-bound); AMD via **ADLX** (ADL is legacy).
8. **Memory relief (≤ 8 GB RAM):** standby list purge **only** when free physical memory stays below a threshold (e.g. < 400 MB) during gameplay, rate-limited; marked `DISPUTED`, must prove itself via §5.6.
9. **Pagefile sanity (PERSISTENT, reboot, consent):** ensure a pagefile exists on the fastest SSD with a sane minimum to prevent OOM crashes. Never disable it.
10. **Disputed / version-dependent (Expert mode only, off by default, must pass verification):** timer resolution (per-process since Win10 2004, ignored for occluded windows on Win11, most games already set it), core parking changes, memory compression toggling (PERSISTENT, reboot).
11. **Game config patcher:** see §5.5.

Explicitly **out of scope** (snake oil or dangerous): registry "cleaners", RAM "boosters" that empty working sets periodically, disabling the pagefile, `bcdedit` timer hacks (`useplatformclock`, `disabledynamictick`, `useplatformtick`), blanket "disable all services" packs, network "TCP optimizer" registry packs, disabling security features (Defender, VBS/HVCI, Spectre mitigations) — at most, inform the user of the trade-off. These are typical of the tweaks that make PCs *worse*; the app may detect them if already applied by other tools and offer to restore defaults.

### 5.3b BIOS Coach — guided firmware improvements (proven useful by the author)
The biggest real gains on many PCs live in the BIOS (RAM speed/XMP/EXPO), which software must not change directly. Overdrive turns it into a safe, guided flow for non-technical users:
1. **Detect:** RAM running below rated speed (SMBIOS `ConfiguredClockSpeed` < `Speed`, or below the platform's JEDEC/XMP profile), motherboard vendor/model and BIOS version (`Win32_BaseBoard`, `Win32_BIOS`), CPU memory-controller limits.
2. **Explain the expected benefit** honestly (e.g. "RAM at 2133 instead of 3200 MHz: often a large 1 % low improvement on Ryzen/iGPU systems").
3. **Guide:** vendor-specific step-by-step (BIOS key, menu path, setting name: XMP / EXPO / DOCP / A-XMP), with a one-click "restart into BIOS/UEFI" (`shutdown /r /fw`). Recommend the **rated profile or one step below it** for stability, never manual overclocking beyond the module rating.
4. **Verify after reboot:** confirm the new speed is active, then run a stability watch: WHEA-Logger / unexpected-shutdown / BSOD events in the following days and an optional short memory stress check. If instability appears, guide the user back one step or to "Load optimized defaults".
5. **Measure:** run the A/B protocol (§5.6) to show the real gain in the report.
Same coach pattern applies to other firmware items when detected: Resizable BAR off on supported hardware, iGPU shared memory (UMA frame buffer) too small, outdated BIOS with known performance/stability fixes (inform only).

### 5.4 Module F — Safety Policy
- **Anti-cheat red line:** maintained database + heuristics (installed drivers/services, game folder signatures) for Vanguard, Easy Anti-Cheat, BattlEye, Ricochet, FACEIT, EA Javelin, nProtect GameGuard, XIGNCODE, and others. For protected titles:
  - never open handles to the game process, never inject, never modify protected game files;
  - only OS-level, out-of-process tweaks (IFEO PerfOptions, power, background isolation, driver profiles, Windows settings).
- **Process suspension policy (lesson from v0.2.1):**
  - **Never suspend:** audio stack and anything currently producing/capturing audio, voice chat in an active call (Discord, TeamSpeak…), the game's launcher/DRM client (Steam, Epic, EA App, Ubisoft Connect, Battle.net…), anti-cheat services, accessibility tools, input/peripheral software, security software, system/shell processes, the foreground window owner, and anything the user whitelisted.
  - Prefer **lowering priority / EcoQoS / trimming** over suspension; suspend only clearly idle, known-safe categories (e.g. updaters, background browsers without media playback) and only with a user-visible list.
  - Every suspension is journaled; resume is guaranteed on game exit, on service stop, on crash recovery and from a one-click "Resume everything" in the UI/tray.
  - A suspended process that the user brings to the foreground is resumed immediately.
- **Risk gating:** Easy mode only applies `LOW` risk + `PROVEN` tweaks. Expert mode exposes everything with explicit risk labels.
- **Restore point** before the first PERSISTENT change, with a short timeout and a non-blocking failure path (lesson from v0.1.1).

### 5.5 Game Config Patcher
- Driven by a **per-game, versioned config database** (not generic per-engine rules): file locations, which keys the game actually honors, safe values, whether cloud saves overwrite the file, anti-cheat status.
- Generic engine rules (e.g. UE `r.Streaming.PoolSize` sized from VRAM budget, disabling chromatic aberration / volumetric fog / heavy AO) are allowed **only** as Expert suggestions with a preview diff.
- Always: backup original, write atomically (temp file + rename), preserve encoding/line endings, detect when the game rewrites the file, respect read-only flags.

### 5.6 Module E — Session Orchestrator & Verification Loop (the core differentiator)
1. Detect game launch (ETW) → apply SESSION tweaks from the active profile.
2. **Baseline protocol:** first sessions (or a guided benchmark) capture a baseline. Comparisons require same game, similar scene, same resolution/settings, minimum duration (e.g. ≥ 60 s of gameplay excluding loading), ≥ 3 runs where guided.
3. **A/B verification:** a tweak is "kept" only if 1 % low or frametime stdev improves beyond the measured run-to-run noise (report the confidence interval). Otherwise revert and mark "no measurable benefit on this PC".
4. Detect game exit → revert SESSION tweaks → verify reverted → write session report.
5. Learn per game + per machine: the profile converges to the set of tweaks that proved useful.
6. **Promotion to persistent:** a tweak that proves beneficial across several sessions (and is safe as a permanent setting) is offered as a permanent change in one plain-language prompt ("This made Fortnite smoother 4 times in a row. Keep it always?"). Tweaks that made things **worse** are blacklisted for that machine and shown in the report.

### 5.8 Module I — Drift Guardian & Long-term Health
- **Drift detection:** periodically (event-driven where possible: after Windows Update, driver install, reboot) verify that every persistent change is still in place and still beneficial. Windows feature updates, GPU driver updates and OEM utilities commonly reset power plans, GPU preferences, driver profiles and gaming settings.
- **Repair:** re-apply drifted changes automatically if they are LOW risk and previously approved; otherwise ask once, in plain language.
- **Performance regression watch:** compare recent session metrics against the machine's best-known baseline per game; if 1 % lows drop significantly, diagnose why (new driver, new background app, thermals, disk filling up) and explain it.
- **Long-term health signals:** SSD free-space trend, new startup apps added since last check, rising thermal throttling over weeks (dust/thermal paste hint), pending reboots, outdated GPU driver vs. a known-good version (never auto-install drivers without consent; offer rollback to the last known-good driver if a new one regressed).
- **Monthly "PC health" summary:** one screen, plain language: what Overdrive kept fixed, what changed, one recommended action.

### 5.7 Module D — Transaction Engine ("atomic" defined precisely)
- **Write-ahead journal** (SQLite in WAL mode or append-only JSONL with fsync): the original state is persisted **before** each change.
- Transactions group changes; each step has `apply / verify / revert / verify_revert`, all idempotent.
- Covered state: registry values (including "key did not exist"), service start types/states, power plan GUID/mode, file contents (hash + backup), process priorities/suspensions, driver profile settings.
- **Recovery:** on service start, any open transaction is rolled back before anything else. On revert failure: retry, then surface a precise, actionable error, never silently leave the system modified.
- **Global reset:** one action reverts every change Overdrive ever made, verified by diffing against the journal.
- Acceptance test: kill the service (and power-cycle a VM) at every step of a session; after restart the registry/services/files must be byte-identical to the pre-session snapshot.

## 6. UI/UX — "Liquid Telemetry"

**Aesthetic:** Teenage Engineering / Nothing OS / aerospace telemetry. No red/neon-green gamer clichés. Borderless, high-contrast dark + light themes, subtle glass, Inter or Geist Sans + JetBrains Mono for live metrics. 60 fps micro-animations that respect "reduce motion" and degrade gracefully on iGPUs (the UI itself must not cost FPS: no animation runs while a game is active).

**Rendering rule (lesson from v0.1.2):** every UI state is derived from engine state received over IPC. No optimistic overlays that can get stuck; every long operation shows real progress, elapsed time and a defined outcome.

- **Pulse hero:** a single generative visualizer (waveform/mesh) reflecting real state: idle, bottleneck type, thermal pressure, session active. It is informative, not decorative: hovering reveals the underlying numbers.
- **Layer 1 — Default:** detected game library + one magnetic "Optimize" action + a plain-language diagnostic badge, e.g. *"Limited by: 2 GB VRAM · 3 fixes ready to test"* (no numeric promises before measurement).
- **Layer 2 — Plan preview:** expands into a Before → After blueprint: each tweak with plain-language why, scope (session/persistent), risk, evidence status and an individual toggle.
- **Layer 3 — Live HUD (optional) & Post-session report:** baseline vs optimized 1 % low, 0.1 % low, frametime graph and stdev, tweaks kept/reverted and why.
- **Structural advice panel:** hardware issues software can't fix (single-channel RAM, XMP off, near-full SSD, thermals), ranked by expected impact.
- **Modes:** keep Easy / Expert from v0.2.x and map them to the risk gating in §5.4.
- **Tray:** session status, "Resume everything", "Revert everything".
- **Accessibility:** keyboard navigable, screen-reader labels, WCAG AA contrast in both themes.

## 7. Growth & viral hooks (privacy-first)

1. **Before/After Frametime Cards:** auto-generated minimal image (frametime graph, 1 % low delta, hardware tier, game) — only from *measured* sessions, with a verification hash so cards can't be faked trivially.
2. **Community-verified presets:** profiles per game × hardware fingerprint, ranked by aggregated *measured* improvement across users; a preset only shows a gain once enough independent measurements agree.
3. **"Your PC's biggest upgrade" report:** a shareable summary of the structural advice (e.g. "+1 RAM stick = biggest win") — useful, honest and inherently shareable.
4. **"Help a friend" mode:** a non-technical user generates a redacted diagnostic code/link; their tech-savvy friend opens it in Overdrive (Expert view), reviews the plan and sends back a recommended profile the friend applies with one click. Turns every tech-savvy user into a distribution channel.

All sharing and telemetry upload is **opt-in**, anonymized (hardware fingerprint hashed, no usernames/paths/process lists), with a visible "what we send" preview.

## 8. Quality, security and release

- **Code signing** of all binaries and installers (avoids SmartScreen warnings and antivirus false positives, which are common for tools that suspend processes and edit the registry). Plan reputation building.
- **Update channel:** signed manifests, staged rollout, automatic rollback of a bad update.
- **Testing:** unit tests per module with fakes; recorded PresentMon traces for the classifier; VM-based integration tests for apply/revert/crash-recovery on Win10 and Win11; a small real-hardware matrix (iGPU laptop, Optimus laptop, entry desktop).
- **Observability:** structured local logs with rotation, a one-click "export diagnostics" bundle (redacted).
- **Footprint budgets (§4.3) enforced in CI.**

## 9. Open decisions (defaults apply unless changed)

| Decision | Default |
|---|---|
| Final stack | Rust + Tauri 2 (decided in Phase 1, ADR-001) |
| Migration | Parallel build: v0.2.x frozen (one cleanup release v0.2.2), 1.0 built in `crates/` + `ui/`, migration via the 1.0 installer — see docs/architecture/ADR-001-stack.md |
| UI languages | ES + EN |
| Journal storage | SQLite (WAL) |
| Telemetry upload | Off by default |

---

# PART II — PHASE PROMPTS

Send **Part I + one phase**. Each phase must end with its Definition of Done (DoD) met.

## Phase 1 — Architecture & stack decision
Produce: architecture diagram (service / UI / watchdog / IPC), module boundaries and interfaces (Rust traits or equivalent), IPC message schema, journal schema, stack decision record (Tauri vs .NET AOT vs keep Electron) including migration plan from the current codebase (which v0.2.1 modules map where), risk register.
**DoD:** every module in §4.4 has an interface; every past bug in §1 is mapped to the design element that prevents it; footprint budgets have a measurement method.

## Phase 2 — UI prototype with mock engine
Produce: complete, runnable frontend (Layer 1/2/3, Pulse, structural advice, tray states, Easy/Expert, dark/light, ES/EN) driven by a **mock engine that speaks the real IPC schema** from Phase 1, including failure and slow-operation scenarios.
**DoD:** every UI state is reachable from mock engine state; no stuck overlays; reduced-motion respected; works at 1366×768 and on an iGPU without dropping frames.

## Phase 3 — Hardware profiler
Produce: production native code for §5.1 with data source + confidence per field, Tier classification, structural-issues ranking, the BIOS Coach detection + guided flow + post-reboot stability verification (§5.3b), fake providers and tests.
**DoD:** outputs a valid `HardwareProfile` on Win10 and Win11 test machines; no kernel drivers; runs in < 3 s; unknowns reported as `unknown`, never guessed.

## Phase 4 — Telemetry & bottleneck classifier
Produce: PresentMon/ETW integration, metric computation, classifier (§5.2) with hysteresis and loading-screen exclusion, a library of recorded traces and tests.
**DoD:** classifier matches the expected label on all recorded traces; telemetry overhead within §4.3 budget.

## Phase 5 — Transaction engine & safety policy
Produce: journal, transaction state machine, idempotent apply/verify/revert for each state type in §5.7, crash recovery, global reset, suspension policy and anti-cheat checks (§5.4).
**DoD:** the kill-at-every-step VM test passes; a suspended process is always resumed in every exit path; no handle is ever opened to an anti-cheat-protected process.

## Phase 6 — Optimization catalog & verification loop
Produce: the declarative catalog (§5.3) starting with items 1–6, the session orchestrator and the A/B verification protocol (§5.6), promotion to persistent, session reports, and the Drift Guardian (§5.8).
**DoD:** each catalog entry has evidence status, risk, scope and tests; tweaks without measurable benefit are auto-reverted and reported as such; tweaks that hurt are blacklisted per machine; a simulated Windows/driver update that resets settings is detected and repaired.

## Phase 7 — Game config patcher & driver profiles
Produce: per-game config database format + first entries, atomic patcher (§5.5), NVAPI DRS and ADLX integrations.
**DoD:** original files restored byte-identical on revert; protected titles never touched.

## Phase 8 — Sharing, community presets & release
Produce: frametime cards, community preset model with verification, privacy preview, code signing and update pipeline, CI footprint checks.
**DoD:** nothing leaves the machine without opt-in; cards and presets only show measured data; signed release installs cleanly on a fresh Win10 and Win11 VM.
