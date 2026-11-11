'use strict';
const { app, BrowserWindow, ipcMain, shell } = require('electron');
const path = require('path');
const { psRun } = require('./ps');
const { analyzeHardware, analyzeDrivers } = require('./system');
const { tweaksForLevel, getTweak } = require('./tweaks');
const { sampleMetrics } = require('./monitor');
const { initUpdater, checkForUpdates, quitAndInstall } = require('./updater');
const profiles = require('./profiles');
const games = require('./games');
const { suspend, resume } = require('./suspend');
const benchmark = require('./benchmark');

let mainWindow = null;

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1120,
    height: 760,
    minWidth: 900,
    minHeight: 640,
    backgroundColor: '#0a0b10',
    title: 'Overdrive',
    autoHideMenuBar: true,
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
    },
  });

  mainWindow.loadFile(path.join(__dirname, '..', 'renderer', 'index.html'));
  if (process.argv.includes('--dev')) mainWindow.webContents.openDevTools({ mode: 'detach' });

  // Auto-updater: inicializa y comprueba al arrancar (solo en app empaquetada).
  initUpdater(mainWindow);
  mainWindow.webContents.once('did-finish-load', () => {
    setTimeout(() => checkForUpdates(), 3000);
  });
}

app.whenReady().then(() => {
  createWindow();
  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
  });
});

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit();
});

// ------------------------------- IPC -------------------------------

ipcMain.handle('system:analyze', async () => {
  return await analyzeHardware();
});

ipcMain.handle('system:drivers', async () => {
  return await analyzeDrivers();
});

// Devuelve los tweaks del nivel con su estado actual (on/off/unknown).
ipcMain.handle('tweaks:list', async (_e, { level, caps }) => {
  const hasSSD = caps && caps.hasSSD;
  const vendors = (caps && caps.gpuVendors) || [];
  const list = tweaksForLevel(level).filter((t) => {
    if (t.ssdOnly && !hasSSD) return false;
    if (t.nvidiaOnly && !vendors.includes('nvidia')) return false;
    if (t.amdOnly && !vendors.includes('amd')) return false;
    return true;
  });
  const withState = await Promise.all(
    list.map(async (t) => {
      let state = 'unknown';
      try {
        const out = (await psRun(t.check, { timeout: t.timeout || 60000 })).trim();
        if (out === 'on' || out === 'off') state = out;
      } catch (_) {
        state = 'unknown';
      }
      return {
        id: t.id,
        title: t.title,
        desc: t.desc,
        category: t.category,
        risk: t.risk,
        needsAdmin: !!t.needsAdmin,
        needsReboot: !!t.needsReboot,
        irreversible: !!t.irreversible,
        isInstall: !!t.isInstall,
        informational: !!t.informational,
        state,
      };
    })
  );
  return withState;
});

// Crea un punto de restauracion del sistema. Requiere admin y Restauracion activada.
ipcMain.handle('system:restorePoint', async () => {
  // No intentamos Enable-ComputerRestore (es el paso lento que bloqueaba).
  // Solo creamos el checkpoint: si Restaurar Sistema esta desactivado,
  // Checkpoint-Computer falla rapido y seguimos sin punto de restauracion.
  const script = `
    try {
      New-ItemProperty 'HKLM:\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\SystemRestore' -Name 'SystemRestorePointCreationFrequency' -Value 0 -PropertyType DWord -Force | Out-Null
      Checkpoint-Computer -Description 'Overdrive Optimizer' -RestorePointType 'MODIFY_SETTINGS' -ErrorAction Stop
      'ok'
    } catch {
      "error: $($_.Exception.Message)"
    }`;
  let out;
  try {
    out = (await psRun(script, { timeout: 60000 })).trim();
  } catch (e) {
    return { ok: false, disabled: true, message: 'Restaurar Sistema está desactivado o tardó demasiado' };
  }
  if (out.startsWith('ok')) return { ok: true };
  return { ok: false, disabled: true, message: out.replace(/^error:\s*/, '') };
});

// Aplica una lista de tweaks (por id), emitiendo progreso.
ipcMain.handle('tweaks:apply', async (e, { ids }) => {
  const results = [];
  for (let i = 0; i < ids.length; i++) {
    const t = getTweak(ids[i]);
    if (!t) continue;
    e.sender.send('tweaks:progress', { id: t.id, title: t.title, index: i, total: ids.length, isInstall: !!t.isInstall });
    try {
      const out = (await psRun(t.apply, { timeout: t.timeout || 120000 })).trim();
      results.push({ id: t.id, ok: true, needsReboot: !!t.needsReboot, out });
    } catch (err) {
      results.push({ id: t.id, ok: false, message: err.message });
    }
  }
  e.sender.send('tweaks:progress', { done: true, total: ids.length });
  return results;
});

// Revierte una lista de tweaks (por id).
ipcMain.handle('tweaks:revert', async (e, { ids }) => {
  const results = [];
  for (const id of ids) {
    const t = getTweak(id);
    if (!t || t.irreversible) continue;
    try {
      const out = (await psRun(t.revert, { timeout: t.timeout || 120000 })).trim();
      results.push({ id, ok: true, out });
    } catch (err) {
      results.push({ id, ok: false, message: err.message });
    }
  }
  return results;
});

// Monitor en tiempo real: devuelve una muestra de métricas.
ipcMain.handle('monitor:sample', async () => {
  return await sampleMetrics();
});

// Auto-updater
ipcMain.handle('update:check', async () => {
  await checkForUpdates();
  return true;
});
ipcMain.handle('update:install', async () => {
  quitAndInstall();
  return true;
});

ipcMain.handle('app:openExternal', async (_e, url) => {
  await shell.openExternal(url);
  return true;
});

// ------------------------- Perfiles / juegos -------------------------
ipcMain.handle('profiles:list', async () => profiles.all());
ipcMain.handle('profiles:save', async (_e, p) => profiles.upsert(p));
ipcMain.handle('profiles:delete', async (_e, id) => { profiles.remove(id); return true; });
ipcMain.handle('profiles:export', async (_e, id) => profiles.exportCode(id));
ipcMain.handle('profiles:import', async (_e, code) => profiles.importCode(code));
ipcMain.handle('games:steam', async () => games.steamGames());
ipcMain.handle('games:running', async () => games.runningProcesses());
ipcMain.handle('games:suspendCandidates', async () => games.suspendCandidates());

// Aplicar / revertir un perfil manualmente (sin esperar al vigilante).
ipcMain.handle('profiles:applyNow', async (_e, id) => {
  const p = profiles.get(id);
  if (!p) return { ok: false };
  const r = await applyProfile(p);
  activeProfiles.add(id);
  return { ok: true, freedMB: r.freedMB };
});
ipcMain.handle('profiles:revertNow', async (_e, id) => {
  const p = profiles.get(id);
  if (!p) return { ok: false };
  await revertProfile(p);
  activeProfiles.delete(id);
  return { ok: true };
});

// ------------------------- Benchmark -------------------------
ipcMain.handle('bench:run', async () => benchmark.run());

// Relanza la app con privilegios de administrador (UAC).
ipcMain.handle('app:relaunchAdmin', async () => {
  const exe = process.execPath;
  // En app empaquetada NO se pasa -ArgumentList (un '' vacío hace fallar Start-Process).
  // En desarrollo hay que pasar la ruta de la app como argumento.
  const script = process.defaultApp
    ? `Start-Process -FilePath '${exe}' -ArgumentList '${app.getAppPath()}' -Verb RunAs`
    : `Start-Process -FilePath '${exe}' -Verb RunAs`;
  try {
    await psRun(script);
    app.quit();
    return { ok: true };
  } catch (err) {
    // Incluye el caso de que el usuario rechace el UAC.
    return { ok: false, message: err.message };
  }
});

// ------------------------- Perfiles: aplicar/revertir + vigilante -------------------------
const activeProfiles = new Set();

async function applyProfile(p) {
  for (const id of p.tweaks || []) {
    const t = getTweak(id);
    if (t) { try { await psRun(t.apply, { timeout: t.timeout || 120000 }); } catch (_) {} }
  }
  if (p.gpuMax) {
    const t = getTweak('nvidia-max-perf');
    if (t) { try { await psRun(t.apply); } catch (_) {} }
  }
  let freedMB = 0;
  if (p.suspend && p.suspend.length) {
    const r = await suspend(p.suspend);
    freedMB = r.freedMB || 0;
  }
  return { freedMB };
}

async function revertProfile(p) {
  for (const id of p.tweaks || []) {
    const t = getTweak(id);
    if (t && !t.irreversible) { try { await psRun(t.revert, { timeout: t.timeout || 120000 }); } catch (_) {} }
  }
  if (p.gpuMax) {
    const t = getTweak('nvidia-max-perf');
    if (t) { try { await psRun(t.revert); } catch (_) {} }
  }
  if (p.suspend && p.suspend.length) await resume(p.suspend);
}

function notifyRenderer(channel, data) {
  if (mainWindow && !mainWindow.isDestroyed()) mainWindow.webContents.send(channel, data);
}

// Nombres de proceso en ejecución (sin extensión, en minúsculas).
async function runningProcNames() {
  try {
    const out = (await psRun(`Get-Process | Select-Object -ExpandProperty ProcessName -Unique`, { timeout: 15000 })).trim();
    return new Set(out.split(/\r?\n/).map((s) => s.trim().toLowerCase()).filter(Boolean));
  } catch (_) {
    return new Set();
  }
}

function baseName(exe) {
  return String(exe || '').toLowerCase().replace(/\.exe$/, '');
}

let watcherTimer = null;
async function pollGames() {
  const list = profiles.all().filter((p) => p.autoApply && p.exe);
  if (!list.length) return;
  const running = await runningProcNames();
  for (const p of list) {
    const isUp = running.has(baseName(p.exe));
    if (isUp && !activeProfiles.has(p.id)) {
      activeProfiles.add(p.id);
      const r = await applyProfile(p);
      notifyRenderer('profile:event', { type: 'applied', id: p.id, name: p.name, freedMB: r.freedMB });
    } else if (!isUp && activeProfiles.has(p.id)) {
      activeProfiles.delete(p.id);
      await revertProfile(p);
      notifyRenderer('profile:event', { type: 'reverted', id: p.id, name: p.name });
    }
  }
}

function startWatcher() {
  if (watcherTimer) return;
  watcherTimer = setInterval(() => { pollGames().catch(() => {}); }, 6000);
}

app.whenReady().then(startWatcher);
