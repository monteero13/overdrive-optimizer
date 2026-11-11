'use strict';
const { app, BrowserWindow, ipcMain, shell } = require('electron');
const path = require('path');
const { psRun } = require('./ps');
const { analyzeHardware, analyzeDrivers } = require('./system');
const { tweaksForLevel, getTweak } = require('./tweaks');
const { sampleMetrics } = require('./monitor');
const { initUpdater, checkForUpdates, quitAndInstall } = require('./updater');

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
  const script = `
    try {
      Enable-ComputerRestore -Drive "$env:SystemDrive\\" -ErrorAction SilentlyContinue
      # Saltar el limite de 1 punto cada 24h para esta operacion puntual.
      New-ItemProperty 'HKLM:\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\SystemRestore' -Name 'SystemRestorePointCreationFrequency' -Value 0 -PropertyType DWord -Force | Out-Null
      Checkpoint-Computer -Description 'Overdrive Optimizer' -RestorePointType 'MODIFY_SETTINGS' -ErrorAction Stop
      'ok'
    } catch {
      "error: $($_.Exception.Message)"
    }`;
  let out;
  try {
    out = (await psRun(script, { timeout: 75000 })).trim();
  } catch (e) {
    return { ok: false, message: 'tardó demasiado o Restaurar Sistema está desactivado' };
  }
  if (out.startsWith('ok')) return { ok: true };
  return { ok: false, message: out.replace(/^error:\s*/, '') };
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

// Relanza la app con privilegios de administrador (UAC).
ipcMain.handle('app:relaunchAdmin', async () => {
  const exe = process.execPath;
  const appPath = app.getAppPath();
  const args = process.defaultApp ? `'${appPath}'` : '';
  const script = `Start-Process -FilePath '${exe}' -ArgumentList ${args ? args : "''"} -Verb RunAs`;
  try {
    await psRun(script);
    app.quit();
    return { ok: true };
  } catch (err) {
    return { ok: false, message: err.message };
  }
});
