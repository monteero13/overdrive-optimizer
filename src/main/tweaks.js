'use strict';
// Catalogo de optimizaciones. Cada tweak es REVERSIBLE y declara:
//  - levels: en que niveles de agresividad aparece (basica < media < agresiva, acumulativo)
//  - risk:   low | medium | high  (para avisar al usuario)
//  - needsAdmin: si requiere privilegios de administrador
//  - check:  PS que imprime 'on' | 'off' | 'unknown' (estado actual)
//  - apply / revert: PS que realiza / deshace el cambio
//
// Filosofia: nada se aplica sin confirmacion del usuario y siempre tras un
// punto de restauracion. Preferimos cambios estandar y documentados de Windows.

const TWEAKS = [
  {
    id: 'power-high',
    title: 'Plan de energia de alto rendimiento',
    desc: 'Activa el plan "Alto rendimiento" para evitar que la CPU baje de frecuencia en reposo.',
    category: 'Energia',
    levels: ['basica', 'media', 'agresiva'],
    risk: 'low',
    needsAdmin: false,
    check: `$a = powercfg /getactivescheme; if ($a -match '8c5e7fda') { 'on' } else { 'off' }`,
    apply: `powercfg -setactive 8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c | Out-Null; 'ok'`,
    revert: `powercfg -setactive 381b4222-f694-41f0-9685-ff5bb260df2e | Out-Null; 'ok'`,
  },
  {
    id: 'clean-temp',
    title: 'Limpiar archivos temporales',
    desc: 'Elimina el contenido de las carpetas TEMP del usuario y de Windows para liberar espacio.',
    category: 'Limpieza',
    levels: ['basica', 'media', 'agresiva'],
    risk: 'low',
    needsAdmin: false,
    check: `'unknown'`,
    apply: `
      Remove-Item "$env:TEMP\\*" -Recurse -Force -ErrorAction SilentlyContinue
      Remove-Item "$env:WINDIR\\Temp\\*" -Recurse -Force -ErrorAction SilentlyContinue
      'ok'`,
    revert: `'no-aplica'`, // limpieza no se revierte (operacion segura e irreversible)
    irreversible: true,
  },
  {
    id: 'game-mode',
    title: 'Activar Modo Juego de Windows',
    desc: 'Habilita el Game Mode para priorizar recursos del sistema hacia el juego en primer plano.',
    category: 'Juego',
    levels: ['basica', 'media', 'agresiva'],
    risk: 'low',
    needsAdmin: false,
    check: `$v = (Get-ItemProperty 'HKCU:\\Software\\Microsoft\\GameBar' -Name AutoGameModeEnabled -ErrorAction SilentlyContinue).AutoGameModeEnabled; if ($v -eq 1) { 'on' } else { 'off' }`,
    apply: `
      New-Item 'HKCU:\\Software\\Microsoft\\GameBar' -Force | Out-Null
      Set-ItemProperty 'HKCU:\\Software\\Microsoft\\GameBar' -Name AutoGameModeEnabled -Value 1 -Type DWord
      Set-ItemProperty 'HKCU:\\Software\\Microsoft\\GameBar' -Name AllowAutoGameMode -Value 1 -Type DWord
      'ok'`,
    revert: `Set-ItemProperty 'HKCU:\\Software\\Microsoft\\GameBar' -Name AutoGameModeEnabled -Value 0 -Type DWord; 'ok'`,
  },
  {
    id: 'visual-fx',
    title: 'Efectos visuales: rendimiento',
    desc: 'Ajusta los efectos visuales de Windows al modo "mejor rendimiento" (menos animaciones).',
    category: 'Interfaz',
    levels: ['media', 'agresiva'],
    risk: 'low',
    needsAdmin: false,
    check: `$v = (Get-ItemProperty 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\VisualEffects' -Name VisualFXSetting -ErrorAction SilentlyContinue).VisualFXSetting; if ($v -eq 2) { 'on' } else { 'off' }`,
    apply: `
      New-Item 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\VisualEffects' -Force | Out-Null
      Set-ItemProperty 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\VisualEffects' -Name VisualFXSetting -Value 2 -Type DWord
      'ok'`,
    revert: `Set-ItemProperty 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\VisualEffects' -Name VisualFXSetting -Value 0 -Type DWord; 'ok'`,
  },
  {
    id: 'gamedvr-off',
    title: 'Desactivar Game DVR / grabacion en segundo plano',
    desc: 'Desactiva la captura en segundo plano de la Xbox Game Bar, que puede reducir FPS.',
    category: 'Juego',
    levels: ['media', 'agresiva'],
    risk: 'low',
    needsAdmin: true,
    check: `$v = (Get-ItemProperty 'HKCU:\\System\\GameConfigStore' -Name GameDVR_Enabled -ErrorAction SilentlyContinue).GameDVR_Enabled; if ($v -eq 0) { 'on' } else { 'off' }`,
    apply: `
      Set-ItemProperty 'HKCU:\\System\\GameConfigStore' -Name GameDVR_Enabled -Value 0 -Type DWord -ErrorAction SilentlyContinue
      New-Item 'HKLM:\\SOFTWARE\\Policies\\Microsoft\\Windows\\GameDVR' -Force | Out-Null
      Set-ItemProperty 'HKLM:\\SOFTWARE\\Policies\\Microsoft\\Windows\\GameDVR' -Name AllowGameDVR -Value 0 -Type DWord
      'ok'`,
    revert: `
      Set-ItemProperty 'HKCU:\\System\\GameConfigStore' -Name GameDVR_Enabled -Value 1 -Type DWord -ErrorAction SilentlyContinue
      Remove-ItemProperty 'HKLM:\\SOFTWARE\\Policies\\Microsoft\\Windows\\GameDVR' -Name AllowGameDVR -ErrorAction SilentlyContinue
      'ok'`,
  },
  {
    id: 'hibernate-off',
    title: 'Desactivar hibernacion',
    desc: 'Elimina hiberfil.sys para liberar varios GB de disco (desactiva Inicio rapido).',
    category: 'Limpieza',
    levels: ['media', 'agresiva'],
    risk: 'medium',
    needsAdmin: true,
    check: `if (Test-Path "$env:SystemDrive\\hiberfil.sys") { 'off' } else { 'on' }`,
    apply: `powercfg /h off; 'ok'`,
    revert: `powercfg /h on; 'ok'`,
  },
  {
    id: 'power-throttling-off',
    title: 'Desactivar Power Throttling',
    desc: 'Evita que Windows limite la CPU de procesos en segundo plano por ahorro de energia.',
    category: 'Energia',
    levels: ['media', 'agresiva'],
    risk: 'medium',
    needsAdmin: true,
    check: `$v = (Get-ItemProperty 'HKLM:\\SYSTEM\\CurrentControlSet\\Control\\Power\\PowerThrottling' -Name PowerThrottlingOff -ErrorAction SilentlyContinue).PowerThrottlingOff; if ($v -eq 1) { 'on' } else { 'off' }`,
    apply: `
      New-Item 'HKLM:\\SYSTEM\\CurrentControlSet\\Control\\Power\\PowerThrottling' -Force | Out-Null
      Set-ItemProperty 'HKLM:\\SYSTEM\\CurrentControlSet\\Control\\Power\\PowerThrottling' -Name PowerThrottlingOff -Value 1 -Type DWord
      'ok'`,
    revert: `Set-ItemProperty 'HKLM:\\SYSTEM\\CurrentControlSet\\Control\\Power\\PowerThrottling' -Name PowerThrottlingOff -Value 0 -Type DWord; 'ok'`,
  },
  {
    id: 'gpu-hags',
    title: 'Programacion de GPU acelerada por hardware (HAGS)',
    desc: 'Activa HwSchMode. Puede mejorar latencia en GPUs modernas. Requiere reinicio.',
    category: 'GPU',
    levels: ['agresiva'],
    risk: 'medium',
    needsAdmin: true,
    needsReboot: true,
    check: `$v = (Get-ItemProperty 'HKLM:\\SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers' -Name HwSchMode -ErrorAction SilentlyContinue).HwSchMode; if ($v -eq 2) { 'on' } else { 'off' }`,
    apply: `Set-ItemProperty 'HKLM:\\SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers' -Name HwSchMode -Value 2 -Type DWord; 'ok'`,
    revert: `Set-ItemProperty 'HKLM:\\SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers' -Name HwSchMode -Value 1 -Type DWord; 'ok'`,
  },
  {
    id: 'net-throttling-off',
    title: 'Desactivar Network Throttling (multimedia)',
    desc: 'Pone NetworkThrottlingIndex al maximo para no limitar el trafico de red por multimedia.',
    category: 'Red',
    levels: ['agresiva'],
    risk: 'medium',
    needsAdmin: true,
    check: `$v = (Get-ItemProperty 'HKLM:\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Multimedia\\SystemProfile' -Name NetworkThrottlingIndex -ErrorAction SilentlyContinue).NetworkThrottlingIndex; if ($v -eq 4294967295) { 'on' } else { 'off' }`,
    apply: `Set-ItemProperty 'HKLM:\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Multimedia\\SystemProfile' -Name NetworkThrottlingIndex -Value 0xffffffff -Type DWord; 'ok'`,
    revert: `Set-ItemProperty 'HKLM:\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Multimedia\\SystemProfile' -Name NetworkThrottlingIndex -Value 10 -Type DWord; 'ok'`,
  },
  {
    id: 'sysmain-off',
    title: 'Desactivar SysMain (Superfetch) — solo SSD',
    desc: 'En equipos con SSD, SysMain aporta poco y consume recursos. Desactiva el servicio.',
    category: 'Servicios',
    levels: ['agresiva'],
    risk: 'medium',
    needsAdmin: true,
    ssdOnly: true,
    check: `$s = Get-Service SysMain -ErrorAction SilentlyContinue; if ($s -and $s.Status -eq 'Stopped') { 'on' } else { 'off' }`,
    apply: `Stop-Service SysMain -Force -ErrorAction SilentlyContinue; Set-Service SysMain -StartupType Disabled; 'ok'`,
    revert: `Set-Service SysMain -StartupType Automatic; Start-Service SysMain -ErrorAction SilentlyContinue; 'ok'`,
  },
];

// Devuelve los tweaks aplicables a un nivel dado.
function tweaksForLevel(level) {
  return TWEAKS.filter((t) => t.levels.includes(level));
}

function getTweak(id) {
  return TWEAKS.find((t) => t.id === id);
}

module.exports = { TWEAKS, tweaksForLevel, getTweak };
