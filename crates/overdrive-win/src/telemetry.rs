//! Session telemetry on Windows:
//! - [`PresentMonSource`]: runs PresentMon 2.x (ETW, no injection) for one PID and streams
//!   its CSV stdout through `overdrive_core::telemetry::trace::PresentMonParser`.
//! - [`SystemSampler`]: 1 Hz PDH counters (RAM commit, hard faults, disk queue, CPU perf
//!   limit, dedicated VRAM usage) + battery state.
//!
//! PresentMon (MIT, github.com/GameTechDev/PresentMon) is bundled next to the service as
//! `presentmon\PresentMon.exe` and only runs while a game session is active. ETW capture
//! needs admin rights — the service runs as LocalSystem.

use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use overdrive_core::telemetry::trace::PresentMonParser;
use overdrive_core::telemetry::{FrameSample, SystemSample, TelemetryError, TelemetrySource};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W,
    PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

type CommandFactory = Box<dyn Fn(u32) -> Command + Send>;

pub struct PresentMonSource {
    factory: CommandFactory,
    child: Option<Child>,
    rx: Option<Receiver<FrameSample>>,
    reader: Option<JoinHandle<u64>>,
}

impl PresentMonSource {
    /// Session name lets a crashed service clean up its own ETW session on restart.
    pub const SESSION: &'static str = "OverdriveSession";

    pub fn new(exe: PathBuf) -> Self {
        Self::with_command(Box::new(move |pid| {
            let mut c = Command::new(&exe);
            c.args([
                "--process_id",
                &pid.to_string(),
                "--output_stdout",
                "--session_name",
                Self::SESSION,
                "--stop_existing_session",
                "--terminate_on_proc_exit",
            ]);
            c
        }))
    }

    /// `<dir of current exe>\presentmon\PresentMon.exe`.
    pub fn bundled() -> Result<Self, TelemetryError> {
        let exe =
            std::env::current_exe().map_err(|e| TelemetryError::ToolMissing(e.to_string()))?;
        let pm = exe
            .parent()
            .unwrap_or(&exe)
            .join("presentmon")
            .join("PresentMon.exe");
        if !pm.exists() {
            return Err(TelemetryError::ToolMissing(pm.display().to_string()));
        }
        Ok(Self::new(pm))
    }

    /// Any command whose stdout is PresentMon CSV (tests replay fixtures through `cmd /C type`).
    pub fn with_command(factory: CommandFactory) -> Self {
        Self {
            factory,
            child: None,
            rx: None,
            reader: None,
        }
    }
}

impl TelemetrySource for PresentMonSource {
    fn start(&mut self, pid: u32) -> Result<(), TelemetryError> {
        if self.child.is_some() {
            return Err(TelemetryError::Start("already running".into()));
        }
        let mut cmd = (self.factory)(pid);
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
        let mut child = cmd
            .spawn()
            .map_err(|e| TelemetryError::Start(e.to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| TelemetryError::Start("no stdout".into()))?;
        let (tx, rx) = mpsc::channel();
        self.reader = Some(thread::spawn(move || {
            let mut parser = PresentMonParser::new();
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(f) = parser.push_line(&line) {
                    if tx.send(f).is_err() {
                        break;
                    }
                }
            }
            parser.rejected
        }));
        self.child = Some(child);
        self.rx = Some(rx);
        Ok(())
    }

    fn poll(&mut self) -> Vec<FrameSample> {
        self.rx
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default()
    }

    fn stop(&mut self) -> Result<(), TelemetryError> {
        let mut child = self.child.take().ok_or(TelemetryError::NotRunning)?;
        let _ = child.kill();
        let _ = child.wait();
        if let Some(h) = self.reader.take() {
            let _ = h.join();
        }
        Ok(())
    }
}

impl Drop for PresentMonSource {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

// ---------------------------------------------------------------- system sampler

struct Pdh {
    q: PDH_HQUERY,
    commit: Option<PDH_HCOUNTER>,
    page_reads: Option<PDH_HCOUNTER>,
    disk_queue: Option<PDH_HCOUNTER>,
    perf_limit: Option<PDH_HCOUNTER>,
    vram: Option<PDH_HCOUNTER>,
}

impl Pdh {
    fn open() -> Option<Self> {
        unsafe {
            let mut q = PDH_HQUERY::default();
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut q) != 0 {
                return None;
            }
            let add = |p: &str| {
                let mut c = PDH_HCOUNTER::default();
                (PdhAddEnglishCounterW(q, &HSTRING::from(p), 0, &mut c) == 0).then_some(c)
            };
            let s = Self {
                q,
                commit: add("\\Memory\\% Committed Bytes In Use"),
                page_reads: add("\\Memory\\Page Reads/sec"),
                disk_queue: add("\\PhysicalDisk(_Total)\\Current Disk Queue Length"),
                perf_limit: add("\\Processor Information(_Total)\\% Performance Limit"),
                vram: add("\\GPU Adapter Memory(*)\\Dedicated Usage"),
            };
            PdhCollectQueryData(q);
            Some(s)
        }
    }

    fn value(c: Option<PDH_HCOUNTER>) -> Option<f64> {
        let mut v = PDH_FMT_COUNTERVALUE::default();
        unsafe {
            (PdhGetFormattedCounterValue(c?, PDH_FMT_DOUBLE, None, &mut v) == 0)
                .then_some(v.Anonymous.doubleValue)
        }
    }

    /// Max over instances (one per adapter) of a wildcard counter.
    fn max_instance(c: Option<PDH_HCOUNTER>) -> Option<f64> {
        let c = c?;
        unsafe {
            let (mut size, mut count) = (0u32, 0u32);
            if PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE, &mut size, &mut count, None)
                != PDH_MORE_DATA
                || size == 0
            {
                return None;
            }
            let n = (size as usize).div_ceil(std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>());
            let mut buf = vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); n];
            if PdhGetFormattedCounterArrayW(
                c,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                Some(buf.as_mut_ptr()),
            ) != 0
            {
                return None;
            }
            buf[..count as usize]
                .iter()
                .map(|i| i.FmtValue.Anonymous.doubleValue)
                .reduce(f64::max)
        }
    }

    fn sample(&self, t_ms: f64, vram_total_bytes: Option<f64>) -> SystemSample {
        unsafe { PdhCollectQueryData(self.q) };
        SystemSample {
            t_ms,
            ram_commit_pct: Self::value(self.commit),
            vram_usage_pct: Self::max_instance(self.vram)
                .zip(vram_total_bytes)
                .map(|(u, t)| (u / t * 100.0).min(100.0)),
            hard_faults_per_s: Self::value(self.page_reads),
            disk_queue: Self::value(self.disk_queue),
            perf_limit_pct: Self::value(self.perf_limit),
            on_battery: crate::sys::power_status().and_then(|(ac, _)| ac.map(|a| !a)),
        }
    }
}

impl Drop for Pdh {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.q) };
    }
}

/// Background 1 Hz sampler. Cheap: one PDH collect per tick on a sleeping thread.
pub struct SystemSampler {
    stop: Arc<AtomicBool>,
    rx: Receiver<SystemSample>,
    handle: Option<JoinHandle<SamplerStats>>,
}

impl SystemSampler {
    pub fn start(interval: Duration) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let flag = stop.clone();
        // VRAM total = largest dedicated memory among adapters (the GPU games run on).
        let vram_total = crate::sys::dxgi_adapters()
            .ok()
            .and_then(|a| a.iter().map(|x| x.dedicated_bytes).max())
            .filter(|&b| b > 0)
            .map(|b| b as f64);
        let handle = thread::spawn(move || {
            let mut stats = SamplerStats::default();
            let Some(pdh) = Pdh::open() else { return stats };
            stats.setup_cpu = thread_cpu_time();
            let c0 = thread_cycles();
            let t0 = Instant::now();
            while !flag.load(Ordering::Relaxed) {
                // Sleep in short slices so stop() returns quickly.
                let wake = Instant::now() + interval;
                while Instant::now() < wake && !flag.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(50).min(interval));
                }
                if flag.load(Ordering::Relaxed) {
                    break;
                }
                if tx
                    .send(pdh.sample(t0.elapsed().as_secs_f64() * 1000.0, vram_total))
                    .is_err()
                {
                    break;
                }
            }
            stats.steady_wall = t0.elapsed();
            stats.steady_cpu = thread_cpu_time().saturating_sub(stats.setup_cpu);
            stats.steady_cycles = thread_cycles().saturating_sub(c0);
            stats
        });
        Self {
            stop,
            rx,
            handle: Some(handle),
        }
    }

    pub fn drain(&self) -> Vec<SystemSample> {
        self.rx.try_iter().collect()
    }

    pub fn stop(mut self) -> (Vec<SystemSample>, SamplerStats) {
        self.stop.store(true, Ordering::Relaxed);
        let stats = self
            .handle
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        (self.rx.try_iter().collect(), stats)
    }
}

/// CPU cost of the sampler thread itself (not the whole process), split into the
/// one-time PDH setup and the steady-state sampling loop.
#[derive(Debug, Clone, Copy, Default)]
pub struct SamplerStats {
    pub setup_cpu: Duration,
    /// Coarse (15.6 ms tick) — kept for reference.
    pub steady_cpu: Duration,
    /// Exact CPU cycles spent by the sampler thread in steady state (QueryThreadCycleTime).
    pub steady_cycles: u64,
    pub steady_wall: Duration,
}

impl SamplerStats {
    /// Steady-state CPU as % of one core, from exact cycles at the CPU's nominal clock
    /// (conservative: boost clocks would make the real share even smaller).
    pub fn steady_pct(&self) -> f64 {
        let mhz = crate::sys::reg_dword(
            crate::sys::Root::Machine,
            &crate::sys::UserHive::CurrentUser,
            "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0",
            "~MHz",
        )
        .unwrap_or(2000) as f64;
        if self.steady_wall.is_zero() {
            return 0.0;
        }
        self.steady_cycles as f64 / (mhz * 1e6 * self.steady_wall.as_secs_f64()) * 100.0
    }
}

/// Exact cycles consumed by the calling thread.
pub fn thread_cycles() -> u64 {
    use windows::Win32::System::Threading::GetCurrentThread;
    use windows::Win32::System::WindowsProgramming::QueryThreadCycleTime;
    let mut c = 0u64;
    unsafe {
        let _ = QueryThreadCycleTime(GetCurrentThread(), &mut c);
    }
    c
}

fn filetime(f: windows::Win32::Foundation::FILETIME) -> Duration {
    Duration::from_nanos((((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64) * 100)
}

/// CPU time (user + kernel) consumed by the calling thread.
pub fn thread_cpu_time() -> Duration {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
    let (mut c, mut e, mut k, mut u) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe {
        if GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u).is_err() {
            return Duration::ZERO;
        }
    }
    filetime(k) + filetime(u)
}

/// CPU time (user + kernel) consumed by this process so far — used to verify overhead budgets.
pub fn process_cpu_time() -> Duration {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let (mut c, mut e, mut k, mut u) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe {
        if GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u).is_err() {
            return Duration::ZERO;
        }
    }
    filetime(k) + filetime(u)
}
