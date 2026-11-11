'use strict';
// Detección de juegos instalados (Steam) y procesos en ejecución, para que el
// usuario cree un perfil sin tener que escribir el .exe a mano.

const fs = require('fs');
const path = require('path');
const { psRun } = require('./ps');

// --- Steam ---
function steamPath() {
  const candidates = [
    'C:\\Program Files (x86)\\Steam',
    'C:\\Program Files\\Steam',
  ];
  for (const c of candidates) {
    if (fs.existsSync(path.join(c, 'steamapps'))) return c;
  }
  return null;
}

// Parser minimalista de VDF: extrae todos los valores de una clave dada.
function vdfValues(text, key) {
  const re = new RegExp('"' + key + '"\\s+"([^"]+)"', 'gi');
  const out = [];
  let m;
  while ((m = re.exec(text))) out.push(m[1]);
  return out;
}

function steamLibraries(steam) {
  const libs = new Set([steam]);
  const vdf = path.join(steam, 'steamapps', 'libraryfolders.vdf');
  try {
    const txt = fs.readFileSync(vdf, 'utf8');
    for (const p of vdfValues(txt, 'path')) libs.add(p.replace(/\\\\/g, '\\'));
  } catch (_) {
    /* usa solo la principal */
  }
  return [...libs];
}

// Busca el .exe mas grande en una carpeta (heurística del ejecutable principal).
function biggestExe(dir, depth = 3) {
  let best = null;
  function walk(d, lvl) {
    if (lvl < 0) return;
    let entries;
    try {
      entries = fs.readdirSync(d, { withFileTypes: true });
    } catch (_) {
      return;
    }
    for (const e of entries) {
      const full = path.join(d, e.name);
      if (e.isDirectory()) {
        walk(full, lvl - 1);
      } else if (e.isFile() && e.name.toLowerCase().endsWith('.exe')) {
        // descartar ejecutables tipicos de utilidades
        if (/unins|redist|vcredist|crash|setup|launcher_installer|dxsetup/i.test(e.name)) continue;
        let size = 0;
        try {
          size = fs.statSync(full).size;
        } catch (_) {}
        if (!best || size > best.size) best = { exe: e.name, path: full, size };
      }
    }
  }
  walk(dir, depth);
  return best;
}

function steamGames() {
  const steam = steamPath();
  if (!steam) return [];
  const games = [];
  for (const lib of steamLibraries(steam)) {
    const appsDir = path.join(lib, 'steamapps');
    let files;
    try {
      files = fs.readdirSync(appsDir).filter((f) => /^appmanifest_\d+\.acf$/i.test(f));
    } catch (_) {
      continue;
    }
    for (const f of files) {
      try {
        const txt = fs.readFileSync(path.join(appsDir, f), 'utf8');
        const name = vdfValues(txt, 'name')[0];
        const installdir = vdfValues(txt, 'installdir')[0];
        if (!name || !installdir) continue;
        if (/steamworks|redistributable|proton|steam linux runtime/i.test(name)) continue;
        const gameDir = path.join(appsDir, 'common', installdir);
        const exeInfo = biggestExe(gameDir);
        games.push({
          name,
          source: 'Steam',
          dir: gameDir,
          exe: exeInfo ? exeInfo.exe : '',
        });
      } catch (_) {
        /* saltar manifest ilegible */
      }
    }
  }
  return games;
}

// --- Procesos en ejecución (para elegir un juego ya abierto) ---
async function runningProcesses() {
  const script = `
    $ErrorActionPreference='SilentlyContinue'
    Get-Process | Where-Object { $_.MainWindowTitle -and $_.Path } |
      Sort-Object WorkingSet64 -Descending |
      Select-Object -First 25 @{N='name';E={$_.ProcessName}}, @{N='exe';E={Split-Path $_.Path -Leaf}}, @{N='title';E={$_.MainWindowTitle}} |
      ConvertTo-Json -Compress`;
  try {
    const out = (await psRun(script, { timeout: 15000 })).trim();
    if (!out) return [];
    const data = JSON.parse(out);
    return Array.isArray(data) ? data : [data];
  } catch (_) {
    return [];
  }
}

// Procesos de fondo típicos que se pueden aligerar (recortar RAM / bajar prioridad) durante el juego.
const KNOWN_BACKGROUND = ['chrome', 'msedge', 'firefox', 'brave', 'Discord', 'Spotify', 'OneDrive', 'Telegram', 'Teams', 'Slack', 'steamwebhelper', 'EpicGamesLauncher', 'GoogleDriveFS'];

async function suspendCandidates() {
  const known = KNOWN_BACKGROUND;
  const script = `
    $ErrorActionPreference='SilentlyContinue'
    $names = @(${known.map((n) => `'${n}'`).join(',')})
    Get-Process | Where-Object { $names -contains $_.ProcessName } |
      Group-Object ProcessName | ForEach-Object {
        [ordered]@{ name = $_.Name; count = $_.Count; memMB = [math]::Round((($_.Group | Measure-Object WorkingSet64 -Sum).Sum)/1MB,0) }
      } | ConvertTo-Json -Compress`;
  try {
    const out = (await psRun(script, { timeout: 15000 })).trim();
    if (!out) return [];
    const data = JSON.parse(out);
    return Array.isArray(data) ? data : [data];
  } catch (_) {
    return [];
  }
}

module.exports = { steamGames, runningProcesses, suspendCandidates, KNOWN_BACKGROUND };
