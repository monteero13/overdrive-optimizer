'use strict';
const { contextBridge, ipcRenderer } = require('electron');

// API segura expuesta al renderer. No se expone ipcRenderer directamente.
contextBridge.exposeInMainWorld('overdrive', {
  analyze: () => ipcRenderer.invoke('system:analyze'),
  analyzeDrivers: () => ipcRenderer.invoke('system:drivers'),
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
  // Monitor en tiempo real
  monitorSample: () => ipcRenderer.invoke('monitor:sample'),
  // Auto-updater
  checkUpdates: () => ipcRenderer.invoke('update:check'),
  installUpdate: () => ipcRenderer.invoke('update:install'),
  onUpdateStatus: (cb) => {
    const handler = (_e, data) => cb(data);
    ipcRenderer.on('update:status', handler);
    return () => ipcRenderer.removeListener('update:status', handler);
  },
  // Perfiles / juegos
  listProfiles: () => ipcRenderer.invoke('profiles:list'),
  saveProfile: (p) => ipcRenderer.invoke('profiles:save', p),
  deleteProfile: (id) => ipcRenderer.invoke('profiles:delete', id),
  exportProfile: (id) => ipcRenderer.invoke('profiles:export', id),
  importProfile: (code) => ipcRenderer.invoke('profiles:import', code),
  applyProfileNow: (id) => ipcRenderer.invoke('profiles:applyNow', id),
  revertProfileNow: (id) => ipcRenderer.invoke('profiles:revertNow', id),
  steamGames: () => ipcRenderer.invoke('games:steam'),
  runningProcs: () => ipcRenderer.invoke('games:running'),
  suspendCandidates: () => ipcRenderer.invoke('games:suspendCandidates'),
  suspendProcs: (names) => ipcRenderer.invoke('suspend:do', names),
  resumeProcs: (names) => ipcRenderer.invoke('suspend:undo', names),
  onProfileEvent: (cb) => {
    const handler = (_e, data) => cb(data);
    ipcRenderer.on('profile:event', handler);
    return () => ipcRenderer.removeListener('profile:event', handler);
  },
  // Benchmark
  runBenchmark: () => ipcRenderer.invoke('bench:run'),
});
