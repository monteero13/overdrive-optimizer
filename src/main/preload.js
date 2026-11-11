'use strict';
const { contextBridge, ipcRenderer } = require('electron');

// API segura expuesta al renderer. No se expone ipcRenderer directamente.
contextBridge.exposeInMainWorld('overdrive', {
  analyze: () => ipcRenderer.invoke('system:analyze'),
  listTweaks: (level, caps) => ipcRenderer.invoke('tweaks:list', { level, caps }),
  restorePoint: () => ipcRenderer.invoke('system:restorePoint'),
  apply: (ids) => ipcRenderer.invoke('tweaks:apply', { ids }),
  revert: (ids) => ipcRenderer.invoke('tweaks:revert', { ids }),
  openExternal: (url) => ipcRenderer.invoke('app:openExternal', url),
  relaunchAdmin: () => ipcRenderer.invoke('app:relaunchAdmin'),
  onProgress: (cb) => {
    const handler = (_e, data) => cb(data);
    ipcRenderer.on('tweaks:progress', handler);
    return () => ipcRenderer.removeListener('tweaks:progress', handler);
  },
});
