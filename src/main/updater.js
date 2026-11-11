'use strict';
// Auto-actualización con electron-updater (proveedor: GitHub Releases).
// Solo funciona en la app empaquetada; en desarrollo se informa y no hace nada.

const { app, dialog } = require('electron');
const { autoUpdater } = require('electron-updater');

let win = null;

function send(status, data = {}) {
  if (win && !win.isDestroyed()) {
    win.webContents.send('update:status', { status, ...data });
  }
}

function initUpdater(window) {
  win = window;
  autoUpdater.autoDownload = true;
  autoUpdater.autoInstallOnAppQuit = true;

  autoUpdater.on('checking-for-update', () => send('checking'));
  autoUpdater.on('update-available', (info) => send('available', { version: info.version }));
  autoUpdater.on('update-not-available', () => send('none'));
  autoUpdater.on('download-progress', (p) => send('downloading', { percent: Math.round(p.percent) }));
  autoUpdater.on('update-downloaded', (info) => {
    send('downloaded', { version: info.version });
    // Aviso nativo claro (mas visible que el panel inferior).
    dialog
      .showMessageBox(win, {
        type: 'info',
        buttons: ['Reiniciar e instalar', 'Más tarde'],
        defaultId: 0,
        cancelId: 1,
        title: 'Actualización disponible',
        message: `Overdrive ${info.version} está listo para instalar`,
        detail: 'Se aplicará al reiniciar la aplicación.',
      })
      .then((r) => { if (r.response === 0) autoUpdater.quitAndInstall(); })
      .catch(() => {});
  });
  autoUpdater.on('error', (e) => send('error', { message: e == null ? 'desconocido' : (e.message || String(e)) }));
}

async function checkForUpdates() {
  if (!app.isPackaged) {
    send('dev');
    return;
  }
  try {
    await autoUpdater.checkForUpdates();
  } catch (e) {
    send('error', { message: e.message || String(e) });
  }
}

function quitAndInstall() {
  try {
    autoUpdater.quitAndInstall();
  } catch (_) {
    /* noop */
  }
}

module.exports = { initUpdater, checkForUpdates, quitAndInstall };
