//! Per-game config database (goals.md §5.5): `catalog/games/*.toml`, embedded at build time.
//!
//! Each entry says, for ONE game: where its config files live, which keys the game really
//! reads, safe values, visual impact, how sure we are it helps, whether the launcher's cloud
//! sync or the game itself rewrites the file, and whether the title is protected by an
//! anti-cheat. Protected titles are a hard block: no file of theirs is ever patched, even
//! if anti-cheat detection on the machine says otherwise.
//!
//! Generic engine rules (Unreal `Engine.ini`) are NOT here: they live in the catalog as
//! Expert-only suggestions with a preview diff.

use serde::{Deserialize, Serialize};

use crate::catalog::{Evidence, GameTarget};
use crate::config::{ConfigEdit, ConfigFormat};
use crate::safety::AntiCheat;

const DB_FILES: &[(&str, &str)] = &[
    ("minecraft_java", include_str!("../../../catalog/games/minecraft_java.toml")),
    ("protected", include_str!("../../../catalog/games/protected.toml")),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameAntiCheat {
    None,
    /// Kernel or user-mode anti-cheat: never patch its files.
    Protected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rewrites {
    Never,
    OnExit,
    OnSettingsChange,
}

/// How much the setting changes what the player sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    None,
    Low,
    Visible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// A fixed recommended value (part of the `game_config` tweak).
    Fixed,
    /// The game's own frame limiter: value computed by the `frame_cap` tweak.
    FrameCap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueFormat {
    Int,
    /// `117.000000` (Unreal floats).
    Float6,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameFile {
    pub id: String,
    /// `{appdata}`, `{localappdata}`, `{documents}`, `{install}` are expanded.
    pub path: String,
    pub format: ConfigFormat,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameSetting {
    pub id: String,
    pub file: String,
    #[serde(default)]
    pub section: Option<String>,
    pub key: String,
    #[serde(default = "fixed")]
    pub role: Role,
    /// Required for `fixed`, absent for `frame_cap`.
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub value_format: Option<ValueFormat>,
    /// Numeric setting where lower = lighter: only ever lowered, never raised.
    #[serde(default)]
    pub lower_is_faster: bool,
    pub evidence: Evidence,
    pub quality: Quality,
    /// Plain-language reason (also shown in Expert previews).
    pub note: String,
}

fn fixed() -> Role {
    Role::Fixed
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameEntry {
    pub id: String,
    pub name: String,
    /// Bumped when an entry changes meaning (journaled with each patch).
    pub db_version: u32,
    /// Library ids as the service names them (`steam:730`, `epic:fortnite`, `minecraft:java`).
    pub library_ids: Vec<String>,
    pub anticheat: GameAntiCheat,
    /// The launcher may restore the file from the cloud (our change can silently vanish).
    #[serde(default)]
    pub cloud_sync: bool,
    #[serde(default = "never")]
    pub rewrites: Rewrites,
    /// The game runs under a generic executable (javaw.exe): anything keyed by exe name
    /// (IFEO, driver profiles) would affect other programs too.
    #[serde(default)]
    pub shared_exe: bool,
    #[serde(default)]
    pub file: Vec<GameFile>,
    #[serde(default)]
    pub setting: Vec<GameSetting>,
    pub notes: String,
}

fn never() -> Rewrites {
    Rewrites::Never
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DbFile {
    game: Vec<GameEntry>,
}

/// Interactive user's folders (the service runs as LocalSystem: never use its own).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserDirs {
    pub appdata: Option<String>,
    pub local_appdata: Option<String>,
    pub documents: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GameDb {
    pub games: Vec<GameEntry>,
}

impl GameDb {
    pub fn builtin() -> Self {
        let mut games = Vec::new();
        for (name, text) in DB_FILES {
            let f: DbFile = toml::from_str(text)
                .unwrap_or_else(|e| panic!("catalog/games/{name}.toml is invalid: {e}"));
            games.extend(f.game);
        }
        Self { games }
    }

    pub fn find(&self, game: &GameTarget) -> Option<&GameEntry> {
        self.games
            .iter()
            .find(|g| g.library_ids.iter().any(|id| id.eq_ignore_ascii_case(&game.id)))
    }

    /// May Overdrive write this game's files? Both sources must agree it is unprotected.
    pub fn may_patch(&self, game: &GameTarget) -> bool {
        game.anticheat == AntiCheat::None
            && self.find(game).is_none_or(|g| g.anticheat == GameAntiCheat::None)
    }
}

impl GameEntry {
    pub fn file(&self, id: &str) -> Option<&GameFile> {
        self.file.iter().find(|f| f.id == id)
    }
}

impl GameSetting {
    /// Easy mode: proven, and the player barely notices (quality none/low).
    pub fn easy_mode_ok(&self) -> bool {
        self.evidence == Evidence::Proven && self.quality != Quality::Visible
    }

    /// Would writing our value be an improvement over `current`?
    pub fn wants_change(&self, current: Option<&str>) -> bool {
        let Some(want) = self.value.as_deref() else {
            return false;
        };
        match current {
            None => true,
            Some(cur) if self.lower_is_faster => match (cur.trim().parse::<f64>(), want.parse::<f64>()) {
                (Ok(c), Ok(w)) => c > w,
                _ => false, // not a number we understand: leave it alone
            },
            Some(cur) => cur.trim().trim_matches('"') != want.trim_matches('"'),
        }
    }
}

/// Expand a path template. `None` if a folder it needs is unknown.
pub fn expand(template: &str, dirs: &UserDirs, install: &str) -> Option<String> {
    let mut out = template.to_string();
    for (var, val) in [
        ("{appdata}", dirs.appdata.as_deref()),
        ("{localappdata}", dirs.local_appdata.as_deref()),
        ("{documents}", dirs.documents.as_deref()),
        ("{install}", (!install.is_empty()).then_some(install)),
    ] {
        if out.contains(var) {
            out = out.replace(var, val?.trim_end_matches(['\\', '/']));
        }
    }
    Some(out)
}

pub fn format_value(v: u32, f: Option<ValueFormat>) -> String {
    match f {
        Some(ValueFormat::Float6) => format!("{v}.000000"),
        _ => v.to_string(),
    }
}

/// `setting` as an edit with `value`.
pub fn edit(s: &GameSetting, value: String) -> ConfigEdit {
    ConfigEdit {
        section: s.section.clone(),
        key: s.key.clone(),
        value,
    }
}

/// Unreal Engine project folder from the executable path:
/// `...\<Project>\Binaries\Win64\<Game>.exe` → `<Project>`.
pub fn unreal_project(exe_path: &str) -> Option<String> {
    let parts: Vec<&str> = exe_path.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
    let bin = parts.iter().rposition(|p| p.eq_ignore_ascii_case("Binaries"))?;
    let project = parts.get(bin.checked_sub(1)?)?;
    let platform = parts.get(bin + 1)?;
    (platform.eq_ignore_ascii_case("Win64") && !project.eq_ignore_ascii_case("Engine"))
        .then(|| project.to_string())
}

/// Folders where an Unreal game keeps its user config (UE5 `Windows`, UE4 `WindowsNoEditor`).
pub fn unreal_config_dirs(project: &str, dirs: &UserDirs) -> Vec<String> {
    let Some(local) = dirs.local_appdata.as_deref() else {
        return vec![];
    };
    let base = format!(r"{}\{project}\Saved\Config", local.trim_end_matches('\\'));
    vec![format!(r"{base}\Windows"), format!(r"{base}\WindowsNoEditor")]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Evidence;

    #[test]
    fn database_is_valid_and_consistent() {
        let db = GameDb::builtin();
        assert!(!db.games.is_empty());
        let mut ids = std::collections::HashSet::new();
        for g in &db.games {
            assert!(ids.insert(&g.id), "duplicate game id {}", g.id);
            assert!(!g.library_ids.is_empty(), "{}: no library ids", g.id);
            if g.anticheat == GameAntiCheat::Protected {
                assert!(g.setting.is_empty() && g.file.is_empty(), "{}: protected titles list no patches", g.id);
            }
            for s in &g.setting {
                let f = g.file(&s.file).unwrap_or_else(|| panic!("{}/{}: unknown file {}", g.id, s.id, s.file));
                match s.role {
                    Role::Fixed => assert!(s.value.is_some(), "{}/{}: fixed setting without value", g.id, s.id),
                    Role::FrameCap => assert!(s.value.is_none(), "{}/{}: frame cap value is computed", g.id, s.id),
                }
                if f.format == ConfigFormat::Ini {
                    assert!(s.section.is_some(), "{}/{}: ini key without section", g.id, s.id);
                }
                // Easy mode only gets what is proven AND invisible.
                if s.evidence == Evidence::Proven {
                    assert!(!s.note.is_empty());
                }
            }
        }
    }

    #[test]
    fn templates_and_unreal_paths() {
        let d = UserDirs {
            appdata: Some(r"C:\Users\a\AppData\Roaming".into()),
            local_appdata: Some(r"C:\Users\a\AppData\Local\".into()),
            documents: None,
        };
        assert_eq!(expand(r"{appdata}\.minecraft\options.txt", &d, "").as_deref(), Some(r"C:\Users\a\AppData\Roaming\.minecraft\options.txt"));
        assert_eq!(expand(r"{documents}\My Games\x.ini", &d, ""), None);
        assert_eq!(
            unreal_project(r"D:\Games\Hogwarts Legacy\Phoenix\Binaries\Win64\HogwartsLegacy.exe").as_deref(),
            Some("Phoenix")
        );
        assert_eq!(unreal_project(r"C:\Games\x\game.exe"), None);
        assert_eq!(unreal_project(r"C:\UE\Engine\Binaries\Win64\x.exe"), None);
        assert_eq!(
            unreal_config_dirs("Phoenix", &d)[1],
            r"C:\Users\a\AppData\Local\Phoenix\Saved\Config\WindowsNoEditor"
        );
        assert_eq!(format_value(117, Some(ValueFormat::Float6)), "117.000000");
    }
}
