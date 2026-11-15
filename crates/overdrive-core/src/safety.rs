//! Module F — Safety Policy (goals.md §5.4).
//!
//! Two jobs:
//! 1. **Anti-cheat red line.** Detect protected games; with a kernel (or unknown) anti-cheat
//!    the engine never opens a handle to the game process — only out-of-process changes
//!    (registry/IFEO, power, services, background isolation of *other* apps).
//! 2. **Background isolation policy** (lesson v0.2.1, when "Game Mode" froze people's apps):
//!    *unknown processes are never touched*. Only known, safe categories are lowered
//!    (EcoQoS + BelowNormal) and only known updaters may be suspended.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::engine::{Action, ProcRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcKind {
    Vanguard,
    EasyAntiCheat,
    BattlEye,
    Ricochet,
    Faceit,
    EaJavelin,
    GameGuard,
    Xigncode,
    Vac,
    PunkBuster,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "level", content = "kind", rename_all = "snake_case")]
pub enum AntiCheat {
    None,
    Kernel(AcKind),
    UserMode(AcKind),
    /// We couldn't tell. Treated as kernel-level: when in doubt, stay out.
    Unknown,
}

impl AntiCheat {
    pub fn forbids_process_access(self) -> bool {
        matches!(self, AntiCheat::Kernel(_) | AntiCheat::Unknown)
    }
}

/// Known titles by executable (lowercase). Checked before folder signatures.
const KNOWN_GAMES: &[(&str, AntiCheat)] = &[
    (
        "valorant-win64-shipping.exe",
        AntiCheat::Kernel(AcKind::Vanguard),
    ),
    ("valorant.exe", AntiCheat::Kernel(AcKind::Vanguard)),
    ("leagueclient.exe", AntiCheat::Kernel(AcKind::Vanguard)),
    ("league of legends.exe", AntiCheat::Kernel(AcKind::Vanguard)),
    (
        "fortniteclient-win64-shipping.exe",
        AntiCheat::Kernel(AcKind::EasyAntiCheat),
    ),
    ("r5apex.exe", AntiCheat::Kernel(AcKind::EasyAntiCheat)),
    ("r5apex_dx12.exe", AntiCheat::Kernel(AcKind::EasyAntiCheat)),
    ("eldenring.exe", AntiCheat::Kernel(AcKind::EasyAntiCheat)),
    ("rustclient.exe", AntiCheat::Kernel(AcKind::EasyAntiCheat)),
    ("tslgame.exe", AntiCheat::Kernel(AcKind::BattlEye)),
    ("rainbowsix.exe", AntiCheat::Kernel(AcKind::BattlEye)),
    ("rainbowsix_be.exe", AntiCheat::Kernel(AcKind::BattlEye)),
    ("escapefromtarkov.exe", AntiCheat::Kernel(AcKind::BattlEye)),
    ("gta5.exe", AntiCheat::Kernel(AcKind::BattlEye)),
    ("gta5_enhanced.exe", AntiCheat::Kernel(AcKind::BattlEye)),
    ("cod.exe", AntiCheat::Kernel(AcKind::Ricochet)),
    ("cod24-cod.exe", AntiCheat::Kernel(AcKind::Ricochet)),
    ("bf2042.exe", AntiCheat::Kernel(AcKind::EaJavelin)),
    ("bf6.exe", AntiCheat::Kernel(AcKind::EaJavelin)),
    ("fc25.exe", AntiCheat::Kernel(AcKind::EaJavelin)),
    ("cs2.exe", AntiCheat::UserMode(AcKind::Vac)),
    ("dota2.exe", AntiCheat::UserMode(AcKind::Vac)),
    ("minecraft.windows.exe", AntiCheat::None),
    ("javaw.exe", AntiCheat::None),
    ("hollow_knight.exe", AntiCheat::None),
];

/// Files/folders in a game's install directory that betray an anti-cheat.
const SIGNATURES: &[(&str, AntiCheat)] = &[
    ("easyanticheat", AntiCheat::Kernel(AcKind::EasyAntiCheat)),
    (
        "easyanticheat_eos_setup.exe",
        AntiCheat::Kernel(AcKind::EasyAntiCheat),
    ),
    (
        "start_protected_game.exe",
        AntiCheat::Kernel(AcKind::EasyAntiCheat),
    ),
    ("battleye", AntiCheat::Kernel(AcKind::BattlEye)),
    ("beservice.exe", AntiCheat::Kernel(AcKind::BattlEye)),
    (
        "eaanticheat.installer.exe",
        AntiCheat::Kernel(AcKind::EaJavelin),
    ),
    (
        "eaanticheat.gameservicelauncher.exe",
        AntiCheat::Kernel(AcKind::EaJavelin),
    ),
    ("gameguard", AntiCheat::Kernel(AcKind::GameGuard)),
    ("xigncode", AntiCheat::Kernel(AcKind::Xigncode)),
    ("pnkbstra.exe", AntiCheat::UserMode(AcKind::PunkBuster)),
];

/// Anti-cheat processes/services themselves: never touched, under any circumstance.
const ANTICHEAT_IMAGES: &[&str] = &[
    "vgc.exe",
    "vgtray.exe",
    "vgk.sys",
    "easyanticheat.exe",
    "easyanticheat_eos.exe",
    "beservice.exe",
    "beservice_x64.exe",
    "faceit.exe",
    "faceitservice.exe",
    "faceitclient.exe",
    "eaanticheat.gameservice.exe",
    "gamemon.des",
    "gamemon64.des",
    "xhunter1.sys",
    "pnkbstra.exe",
    "pnkbstrb.exe",
    "randgrid.exe",
    "atvi-randgrid_sr.exe",
];

#[derive(Debug, Clone, Default)]
pub struct GameInfo {
    pub exe: String,
    /// Top-level entries of the install directory (lowercase names).
    pub install_entries: Vec<String>,
}

pub fn anticheat_status(game: &GameInfo) -> AntiCheat {
    let exe = game.exe.to_ascii_lowercase();
    if let Some((_, ac)) = KNOWN_GAMES.iter().find(|(e, _)| *e == exe) {
        return *ac;
    }
    let entries: Vec<String> = game
        .install_entries
        .iter()
        .map(|e| e.to_ascii_lowercase())
        .collect();
    // Kernel-level signatures win over user-mode ones.
    let mut found: Option<AntiCheat> = None;
    for (sig, ac) in SIGNATURES {
        if entries.iter().any(|e| e == sig) {
            if matches!(ac, AntiCheat::Kernel(_)) {
                return *ac;
            }
            found = Some(*ac);
        }
    }
    match found {
        Some(ac) => ac,
        None if entries.is_empty() => AntiCheat::Unknown, // we didn't get to look: don't claim "none"
        None => AntiCheat::None,
    }
}

// ---------------------------------------------------------------- background policy

const SYSTEM_SHELL: &[&str] = &[
    "system",
    "registry",
    "memory compression",
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "svchost.exe",
    "fontdrvhost.exe",
    "dwm.exe",
    "sihost.exe",
    "ctfmon.exe",
    "taskhostw.exe",
    "runtimebroker.exe",
    "explorer.exe",
    "startmenuexperiencehost.exe",
    "shellexperiencehost.exe",
    "textinputhost.exe",
    "searchhost.exe",
    "searchapp.exe",
    "conhost.exe",
    "dllhost.exe",
    "spoolsv.exe",
    "wudfhost.exe",
    "smartscreen.exe",
    "lockapp.exe",
    "applicationframehost.exe",
    "audiodg.exe",
    "msedgewebview2.exe",
    "systemsettings.exe",
    "taskmgr.exe",
    "winstore.app.exe",
];
const SECURITY: &[&str] = &[
    "msmpeng.exe",
    "nissrv.exe",
    "mssense.exe",
    "securityhealthsystray.exe",
    "securityhealthservice.exe",
    "avp.exe",
    "ekrn.exe",
    "egui.exe",
    "mbamservice.exe",
    "mbamtray.exe",
    "bdservicehost.exe",
    "avastui.exe",
    "avgui.exe",
    "mcshield.exe",
    "nortonsecurity.exe",
];
const SECURITY_PREFIX: &[&str] = &[
    "avast",
    "avg",
    "mcafee",
    "norton",
    "kaspersky",
    "bitdefender",
    "eset",
    "malwarebytes",
    "sophos",
    "trend",
];
const LAUNCHERS: &[&str] = &[
    "steam.exe",
    "steamwebhelper.exe",
    "steamservice.exe",
    "gameoverlayui.exe",
    "epicgameslauncher.exe",
    "epicwebhelper.exe",
    "eadesktop.exe",
    "eabackgroundservice.exe",
    "origin.exe",
    "ubisoftconnect.exe",
    "upc.exe",
    "uplaywebcore.exe",
    "battle.net.exe",
    "agent.exe",
    "riotclientservices.exe",
    "riotclientux.exe",
    "riotclientcrashhandler.exe",
    "galaxyclient.exe",
    "gogalaxy.exe",
    "xboxpcapp.exe",
    "gamingservices.exe",
    "gamingservicesnet.exe",
    "rockstarservice.exe",
    "socialclubhelper.exe",
];
const ACCESSIBILITY: &[&str] = &[
    "narrator.exe",
    "magnify.exe",
    "osk.exe",
    "atbroker.exe",
    "nvda.exe",
    "jfw.exe",
];
const PERIPHERAL_PREFIX: &[&str] = &[
    "lghub",
    "logioptions",
    "razer",
    "synapse",
    "steelseries",
    "icue",
    "armourycrate",
    "msi.",
    "wacom",
    "ds4windows",
    "nahimic",
    "rtkaud",
    "realtek",
];
/// Overlays, recording and voice chat: the game or the user is actively using them.
const OVERLAY_VOICE: &[&str] = &[
    "discord.exe",
    "discordptb.exe",
    "discordcanary.exe",
    "nvcontainer.exe",
    "nvidia share.exe",
    "nvidia overlay.exe",
    "nvidia app.exe",
    "gamebar.exe",
    "gamebarftserver.exe",
    "radeonsoftware.exe",
    "amdrssrcext.exe",
    "rtss.exe",
    "msiafterburner.exe",
    "obs64.exe",
    "obs32.exe",
    "ts3client_win64.exe",
    "teamspeak.exe",
    "mumble.exe",
    "zoom.exe",
    "teams.exe",
    "ms-teams.exe",
    "skype.exe",
];
/// Known updaters with no user-facing work: the only things we may suspend.
const SUSPENDABLE: &[&str] = &[
    "googleupdate.exe",
    "microsoftedgeupdate.exe",
    "adobearm.exe",
    "jusched.exe",
    "onedrivestandaloneupdater.exe",
    "updater.exe",
];
/// Known background apps we may lower (EcoQoS + BelowNormal) when idle and silent.
const LOWERABLE: &[&str] = &[
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "opera.exe",
    "opera_gx.exe",
    "brave.exe",
    "vivaldi.exe",
    "onedrive.exe",
    "dropbox.exe",
    "googledrivefs.exe",
    "slack.exe",
    "whatsapp.exe",
    "telegram.exe",
    "outlook.exe",
    "olk.exe",
    "spotify.exe",
    "notion.exe",
    "code.exe",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeverTouch {
    SystemSession,
    SystemOrShell,
    Security,
    Launcher,
    AntiCheat,
    Accessibility,
    Peripheral,
    OverlayOrVoice,
    ProducingAudio,
    Foreground,
    Game,
    Whitelisted,
    Overdrive,
    /// Not in any known-safe category. The default — lesson v0.2.1.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", content = "reason", rename_all = "snake_case")]
pub enum BackgroundDecision {
    NeverTouch(NeverTouch),
    LowerPriority,
    Suspend,
}

#[derive(Debug, Clone)]
pub struct ProcInfo {
    pub process: ProcRef,
    pub session_id: u32,
    pub parent_pid: Option<u32>,
    pub is_foreground: bool,
    pub produces_audio: bool,
    pub has_visible_window: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SafetyPolicy {
    /// The running game (if any) and its anti-cheat status.
    pub game: Option<(ProcRef, AntiCheat)>,
    pub game_children: HashSet<u32>,
    /// Images the user asked us never to touch (lowercase).
    pub user_whitelist: HashSet<String>,
}

fn in_list(img: &str, list: &[&str]) -> bool {
    list.contains(&img)
}
fn has_prefix(img: &str, list: &[&str]) -> bool {
    list.iter().any(|p| img.starts_with(p))
}

impl SafetyPolicy {
    /// Image-only classification (used by the engine guard where live flags aren't known).
    fn static_reason(&self, p: &ProcRef) -> Option<NeverTouch> {
        let img = p.image.to_ascii_lowercase();
        if img.starts_with("overdrive") {
            return Some(NeverTouch::Overdrive);
        }
        if self.user_whitelist.contains(&img) {
            return Some(NeverTouch::Whitelisted);
        }
        if let Some((g, _)) = &self.game {
            if g.pid == p.pid || self.game_children.contains(&p.pid) {
                return Some(NeverTouch::Game);
            }
        }
        if in_list(&img, ANTICHEAT_IMAGES) {
            return Some(NeverTouch::AntiCheat);
        }
        if in_list(&img, SYSTEM_SHELL) {
            return Some(NeverTouch::SystemOrShell);
        }
        if in_list(&img, SECURITY) || has_prefix(&img, SECURITY_PREFIX) {
            return Some(NeverTouch::Security);
        }
        if in_list(&img, LAUNCHERS) {
            return Some(NeverTouch::Launcher);
        }
        if in_list(&img, ACCESSIBILITY) {
            return Some(NeverTouch::Accessibility);
        }
        if has_prefix(&img, PERIPHERAL_PREFIX) {
            return Some(NeverTouch::Peripheral);
        }
        if in_list(&img, OVERLAY_VOICE) {
            return Some(NeverTouch::OverlayOrVoice);
        }
        None
    }

    pub fn background_decision(&self, p: &ProcInfo) -> BackgroundDecision {
        use BackgroundDecision::*;
        if p.session_id == 0 {
            return NeverTouch(self::NeverTouch::SystemSession);
        }
        if let Some(r) = self.static_reason(&p.process) {
            return NeverTouch(r);
        }
        if p.produces_audio {
            return NeverTouch(self::NeverTouch::ProducingAudio);
        }
        if p.is_foreground {
            return NeverTouch(self::NeverTouch::Foreground);
        }
        let img = p.process.image.to_ascii_lowercase();
        if in_list(&img, SUSPENDABLE) {
            return if p.has_visible_window {
                LowerPriority
            } else {
                Suspend
            };
        }
        if in_list(&img, LOWERABLE) {
            return LowerPriority;
        }
        NeverTouch(self::NeverTouch::Unknown)
    }

    /// Turn a process list into concrete actions (journaled by the engine).
    pub fn background_actions(&self, procs: &[ProcInfo]) -> Vec<Action> {
        let mut out = Vec::new();
        for p in procs {
            match self.background_decision(p) {
                BackgroundDecision::LowerPriority => {
                    out.push(Action::ProcessEcoQos {
                        process: p.process.clone(),
                        on: true,
                    });
                    out.push(Action::ProcessPriority {
                        process: p.process.clone(),
                        class: crate::engine::PriorityClass::BelowNormal,
                    });
                }
                BackgroundDecision::Suspend => out.push(Action::ProcessSuspend {
                    process: p.process.clone(),
                }),
                BackgroundDecision::NeverTouch(_) => {}
            }
        }
        out
    }

    /// Engine guard: may a process action target this process at all?
    /// The game itself is only allowed when its anti-cheat permits process access.
    pub fn check_process(&self, p: &ProcRef) -> Result<(), NeverTouch> {
        if let Some((g, ac)) = &self.game {
            if g.pid == p.pid || self.game_children.contains(&p.pid) {
                return if ac.forbids_process_access() {
                    Err(NeverTouch::AntiCheat)
                } else {
                    Ok(())
                };
            }
        }
        match self.static_reason(p) {
            None | Some(NeverTouch::Game) => Ok(()),
            Some(r) => Err(r),
        }
    }

    /// Split a plan into allowed and blocked actions (blocked never reach the engine).
    pub fn filter_plan(&self, plan: Vec<Action>) -> (Vec<Action>, Vec<(Action, NeverTouch)>) {
        let mut ok = Vec::new();
        let mut blocked = Vec::new();
        for a in plan {
            match a.process().map(|p| self.check_process(p)) {
                Some(Err(r)) => blocked.push((a, r)),
                _ => ok.push(a),
            }
        }
        (ok, blocked)
    }
}

/// What the engine calls before any process primitive (so no handle is ever opened
/// to a protected process, even if a bad plan slips through).
pub trait ProcessGuard {
    fn check(&self, p: &ProcRef) -> Result<(), String>;
}

impl ProcessGuard for SafetyPolicy {
    fn check(&self, p: &ProcRef) -> Result<(), String> {
        self.check_process(p).map_err(|r| format!("{r:?}"))
    }
}
