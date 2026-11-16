//! Game library detection: Steam (`libraryfolders.vdf` + `appmanifest_*.acf`) and Epic
//! (`Manifests\*.item`). Game paths are always resolved here, inside the service. The UI
//! only ever sends a game id (ipc-protocol.md §2.3).
//!
//! The executable is usually not declared (Steam), so it is learned the first time the game
//! runs: the first session of a game is a baseline anyway, with nothing applied.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::platform::LibraryGame;

/// Steam tools that live in the library but are not games.
const STEAM_NON_GAMES: &[&str] = &["228980", "1070560", "1391110", "1628350", "250820"];

/// Every `"key"  "value"` pair of a Valve KeyValues text file, in order.
pub fn vdf_pairs(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut parts = Vec::new();
        let mut cur = String::new();
        let mut inq = false;
        let mut esc = false;
        for c in line.chars() {
            if inq {
                if esc {
                    cur.push(c);
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    parts.push(std::mem::take(&mut cur));
                    inq = false;
                } else {
                    cur.push(c);
                }
            } else if c == '"' {
                inq = true;
            }
        }
        if parts.len() == 2 {
            let v = parts.pop().unwrap();
            let k = parts.pop().unwrap();
            out.push((k.to_ascii_lowercase(), v));
        }
    }
    out
}

/// Library roots listed in `steamapps\libraryfolders.vdf`.
pub fn steam_library_paths(vdf: &str) -> Vec<String> {
    vdf_pairs(vdf)
        .into_iter()
        .filter(|(k, _)| k == "path")
        .map(|(_, v)| v)
        .collect()
}

/// (appid, name, installdir) of one `appmanifest_<id>.acf`.
pub fn steam_manifest(acf: &str) -> Option<(String, String, String)> {
    let pairs = vdf_pairs(acf);
    let get = |k: &str| pairs.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone());
    Some((get("appid")?, get("name")?, get("installdir")?))
}

/// An Epic `.item` manifest (JSON). Skips incomplete installs and non-games.
pub fn epic_manifest(json: &str) -> Option<(String, String, String, Option<String>)> {
    let v: Value = serde_json::from_str(json).ok()?;
    if v.get("bIsIncompleteInstall").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    if let Some(cats) = v.get("AppCategories").and_then(Value::as_array) {
        if !cats.iter().any(|c| c.as_str() == Some("games")) {
            return None;
        }
    }
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
    Some((s("AppName")?, s("DisplayName")?, s("InstallLocation")?, s("LaunchExecutable")))
}

/// Names found under `dir` (lowercase), breadth-first, bounded: enough to spot
/// `EasyAntiCheat\`, `BattlEye\`… without walking a 100 GB install.
pub fn install_entries(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut queue = vec![(dir.to_path_buf(), 0u8)];
    while let Some((d, depth)) = queue.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            if out.len() >= 4000 {
                return out;
            }
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            if depth < 4 && e.file_type().is_ok_and(|t| t.is_dir()) {
                queue.insert(0, (e.path(), depth + 1));
            }
            out.push(name);
        }
    }
    out
}

pub fn scan_steam(steam_root: &Path) -> Vec<LibraryGame> {
    let mut roots = vec![steam_root.to_path_buf()];
    if let Ok(vdf) = fs::read_to_string(steam_root.join("steamapps").join("libraryfolders.vdf")) {
        for p in steam_library_paths(&vdf) {
            let pb = PathBuf::from(p);
            if !roots.iter().any(|r| same_path(r, &pb)) {
                roots.push(pb);
            }
        }
    }
    let mut out = Vec::new();
    for root in roots {
        let apps = root.join("steamapps");
        let Ok(rd) = fs::read_dir(&apps) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_ascii_lowercase();
            if !(n.starts_with("appmanifest_") && n.ends_with(".acf")) {
                continue;
            }
            let Some((appid, name, dir)) = fs::read_to_string(e.path())
                .ok()
                .and_then(|t| steam_manifest(&t))
            else {
                continue;
            };
            if STEAM_NON_GAMES.contains(&appid.as_str()) || name.starts_with("Steamworks") {
                continue;
            }
            let install = apps.join("common").join(&dir);
            if !install.is_dir() {
                continue;
            }
            out.push(LibraryGame {
                id: format!("steam:{appid}"),
                name,
                launcher: "steam",
                install_entries: install_entries(&install),
                install_dir: install.display().to_string(),
                exe_hint: None,
            });
        }
    }
    out
}

/// Minecraft: Java Edition (official launcher). The game runs under the launcher's bundled
/// javaw.exe, so its "install dir" is that Java runtime folder: a javaw.exe from there is
/// Minecraft, any other Java program is not.
pub fn scan_minecraft(appdata: Option<&str>, local_appdata: Option<&str>) -> Vec<LibraryGame> {
    let Some(appdata) = appdata else { return vec![] };
    let game_dir = Path::new(appdata).join(".minecraft");
    if !game_dir.join("options.txt").is_file() && !game_dir.join("versions").is_dir() {
        return vec![];
    }
    let mut runtimes = Vec::new();
    if let Some(local) = local_appdata {
        // Microsoft Store / Xbox app launcher.
        runtimes.push(
            Path::new(local)
                .join(r"Packages\Microsoft.4297127D64EC6_8wekyb3d8bbwe\LocalCache\Local\runtime"),
        );
    }
    runtimes.push(game_dir.join("runtime")); // legacy launcher
    runtimes.push(PathBuf::from(r"C:\Program Files (x86)\Minecraft Launcher\runtime"));
    let Some(runtime) = runtimes.into_iter().find(|p| p.is_dir()) else {
        return vec![];
    };
    vec![LibraryGame {
        id: "minecraft:java".into(),
        name: "Minecraft: Java Edition".into(),
        launcher: "minecraft",
        install_entries: install_entries(&game_dir),
        install_dir: runtime.display().to_string(),
        exe_hint: None,
    }]
}

pub fn scan_epic(manifests: &Path) -> Vec<LibraryGame> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(manifests) else {
        return out;
    };
    for e in rd.flatten() {
        if e.path().extension().and_then(|x| x.to_str()) != Some("item") {
            continue;
        }
        let Some((app, name, dir, exe)) = fs::read_to_string(e.path())
            .ok()
            .and_then(|t| epic_manifest(&t))
        else {
            continue;
        };
        let install = PathBuf::from(&dir);
        if !install.is_dir() {
            continue;
        }
        out.push(LibraryGame {
            id: format!("epic:{}", app.to_ascii_lowercase()),
            name,
            launcher: "epic",
            install_entries: install_entries(&install),
            exe_hint: exe.map(|x| install.join(x).display().to_string()),
            install_dir: install.display().to_string(),
        });
    }
    out
}

fn same_path(a: &Path, b: &Path) -> bool {
    norm(&a.display().to_string()) == norm(&b.display().to_string())
}

/// Lowercase, backslashes, no trailing separator: comparable Windows paths.
pub fn norm(p: &str) -> String {
    let s = p.replace('/', "\\").to_ascii_lowercase();
    s.trim_end_matches('\\').to_string()
}

/// Is `path` inside `dir`?
pub fn under(path: &str, dir: &str) -> bool {
    let (p, d) = (norm(path), norm(dir));
    !d.is_empty() && p.len() > d.len() && p.starts_with(&d) && p.as_bytes()[d.len()] == b'\\'
}

/// Helpers that live next to the game but are not the game.
pub fn is_helper_exe(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    const EXACT: &[&str] = &[
        "start_protected_game.exe",
        "easyanticheat.exe",
        "easyanticheat_eos_setup.exe",
        "beservice.exe",
        "beservice_x64.exe",
        "unitycrashhandler64.exe",
        "unitycrashhandler32.exe",
        "crashreportclient.exe",
        "crashpad_handler.exe",
        "cefprocess.exe",
        "launcher.exe",
        "epicwebhelper.exe",
        "dxsetup.exe",
    ];
    const PARTS: &[&str] = &["unins", "setup", "redist", "vcredist", "vc_redist", "crash", "report", "updater"];
    EXACT.contains(&n.as_str()) || PARTS.iter().any(|p| n.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_steam_library_and_manifest() {
        let vdf = r#""libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"apps" { "228980" "1" }
	}
	"1"
	{
		"path"		"D:\\SteamLibrary"
	}
}"#;
        assert_eq!(
            steam_library_paths(vdf),
            vec![r"C:\Program Files (x86)\Steam", r"D:\SteamLibrary"]
        );
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"730\"\n\t\"name\"\t\t\"Counter-Strike 2\"\n\t\"installdir\"\t\t\"Counter-Strike Global Offensive\"\n}";
        assert_eq!(
            steam_manifest(acf),
            Some((
                "730".into(),
                "Counter-Strike 2".into(),
                "Counter-Strike Global Offensive".into()
            ))
        );
    }

    #[test]
    fn parses_epic_manifest_and_skips_non_games() {
        let game = r#"{"AppName":"Fortnite","DisplayName":"Fortnite","InstallLocation":"C:\\Program Files\\Epic Games\\Fortnite","LaunchExecutable":"FortniteGame/Binaries/Win64/FortniteLauncher.exe","AppCategories":["public","games","applications"],"bIsIncompleteInstall":false}"#;
        let (app, name, dir, exe) = epic_manifest(game).unwrap();
        assert_eq!((app.as_str(), name.as_str()), ("Fortnite", "Fortnite"));
        assert!(dir.ends_with("Fortnite"));
        assert!(exe.unwrap().ends_with("FortniteLauncher.exe"));
        let tool = r#"{"AppName":"UE","DisplayName":"Unreal Engine","InstallLocation":"C:\\UE","AppCategories":["engines"]}"#;
        assert_eq!(epic_manifest(tool), None);
        let partial = r#"{"AppName":"X","DisplayName":"X","InstallLocation":"C:\\X","bIsIncompleteInstall":true}"#;
        assert_eq!(epic_manifest(partial), None);
    }

    #[test]
    fn path_helpers() {
        assert!(under(r"C:\Games\CS2\game\bin\cs2.exe", r"c:/games/cs2/"));
        assert!(!under(r"C:\Games\CS2x\cs2.exe", r"C:\Games\CS2"));
        assert!(!under(r"C:\Games\CS2", r"C:\Games\CS2"));
        assert!(is_helper_exe("UnityCrashHandler64.exe"));
        assert!(is_helper_exe("unins000.exe"));
        assert!(!is_helper_exe("cs2.exe"));
        assert!(!is_helper_exe("FortniteClient-Win64-Shipping.exe"));
    }

    #[test]
    fn finds_minecraft_java_and_its_bundled_runtime() {
        let root = std::env::temp_dir().join(format!("overdrive-mc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (appdata, local) = (root.join("Roaming"), root.join("Local"));
        assert!(scan_minecraft(appdata.to_str(), local.to_str()).is_empty(), "not installed");
        fs::create_dir_all(appdata.join(".minecraft")).unwrap();
        fs::write(appdata.join(".minecraft").join("options.txt"), "maxFps:260\n").unwrap();
        let rt = local.join(r"Packages\Microsoft.4297127D64EC6_8wekyb3d8bbwe\LocalCache\Local\runtime");
        fs::create_dir_all(&rt).unwrap();
        let v = scan_minecraft(appdata.to_str(), local.to_str());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, "minecraft:java");
        assert_eq!(v[0].install_dir, rt.display().to_string(), "a javaw.exe from here is Minecraft");
        let _ = fs::remove_dir_all(&root);
    }
}
