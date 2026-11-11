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
  // -------------------- RATON --------------------
  {
    id: 'mouse-accel-off',
    title: 'Desactivar aceleracion del raton',
    desc: 'Elimina "Mejorar la precision del puntero" para un apuntado 1:1, clave en shooters.',
    category: 'Raton',
    levels: ['basica', 'media', 'agresiva'],
    risk: 'low',
    needsAdmin: false,
    check: `$m = Get-ItemProperty 'HKCU:\\Control Panel\\Mouse'; if ("$($m.MouseSpeed)" -eq '0' -and "$($m.MouseThreshold1)" -eq '0' -and "$($m.MouseThreshold2)" -eq '0') { 'on' } else { 'off' }`,
    apply: `
      Set-ItemProperty 'HKCU:\\Control Panel\\Mouse' -Name MouseSpeed -Value '0'
      Set-ItemProperty 'HKCU:\\Control Panel\\Mouse' -Name MouseThreshold1 -Value '0'
      Set-ItemProperty 'HKCU:\\Control Panel\\Mouse' -Name MouseThreshold2 -Value '0'
      'ok'`,
    revert: `
      Set-ItemProperty 'HKCU:\\Control Panel\\Mouse' -Name MouseSpeed -Value '1'
      Set-ItemProperty 'HKCU:\\Control Panel\\Mouse' -Name MouseThreshold1 -Value '6'
      Set-ItemProperty 'HKCU:\\Control Panel\\Mouse' -Name MouseThreshold2 -Value '10'
      'ok'`,
  },

  // -------------------- ANIMACIONES (granular) --------------------
  {
    id: 'anim-window',
    title: 'Quitar animacion de minimizar/maximizar',
    desc: 'Desactiva la animacion de ventanas al minimizar y maximizar para una respuesta instantanea.',
    category: 'Animaciones',
    levels: ['media', 'agresiva'],
    risk: 'low',
    needsAdmin: false,
    check: `$v = (Get-ItemProperty 'HKCU:\\Control Panel\\Desktop\\WindowMetrics' -Name MinAnimate -ErrorAction SilentlyContinue).MinAnimate; if ("$v" -eq '0') { 'on' } else { 'off' }`,
    apply: `Set-ItemProperty 'HKCU:\\Control Panel\\Desktop\\WindowMetrics' -Name MinAnimate -Value '0'; 'ok'`,
    revert: `Set-ItemProperty 'HKCU:\\Control Panel\\Desktop\\WindowMetrics' -Name MinAnimate -Value '1'; 'ok'`,
  },
  {
    id: 'anim-taskbar',
    title: 'Quitar animaciones de la barra de tareas',
    desc: 'Desactiva las transiciones animadas de la barra de tareas.',
    category: 'Animaciones',
    levels: ['media', 'agresiva'],
    risk: 'low',
    needsAdmin: false,
    check: `$v = (Get-ItemProperty 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced' -Name TaskbarAnimations -ErrorAction SilentlyContinue).TaskbarAnimations; if ($v -eq 0) { 'on' } else { 'off' }`,
    apply: `Set-ItemProperty 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced' -Name TaskbarAnimations -Value 0 -Type DWord; 'ok'`,
    revert: `Set-ItemProperty 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced' -Name TaskbarAnimations -Value 1 -Type DWord; 'ok'`,
  },
  {
    id: 'anim-listview',
    title: 'Quitar fundido de seleccion y sombras de iconos',
    desc: 'Desactiva el desvanecido al seleccionar y las sombras de iconos del explorador.',
    category: 'Animaciones',
    levels: ['media', 'agresiva'],
    risk: 'low',
    needsAdmin: false,
    check: `$a = (Get-ItemProperty 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced' -Name ListviewAlphaSelect -ErrorAction SilentlyContinue).ListviewAlphaSelect; if ($a -eq 0) { 'on' } else { 'off' }`,
    apply: `
      Set-ItemProperty 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced' -Name ListviewAlphaSelect -Value 0 -Type DWord
      Set-ItemProperty 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced' -Name ListviewShadow -Value 0 -Type DWord
      'ok'`,
    revert: `
      Set-ItemProperty 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced' -Name ListviewAlphaSelect -Value 1 -Type DWord
      Set-ItemProperty 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced' -Name ListviewShadow -Value 1 -Type DWord
      'ok'`,
  },
  {
    id: 'anim-dragfull',
    title: 'Quitar contenido al arrastrar ventanas',
    desc: 'Desactiva el redibujado completo al arrastrar ventanas (menos carga grafica).',
    category: 'Animaciones',
    levels: ['media', 'agresiva'],
    risk: 'low',
    needsAdmin: false,
    check: `$v = (Get-ItemProperty 'HKCU:\\Control Panel\\Desktop' -Name DragFullWindows -ErrorAction SilentlyContinue).DragFullWindows; if ("$v" -eq '0') { 'on' } else { 'off' }`,
    apply: `Set-ItemProperty 'HKCU:\\Control Panel\\Desktop' -Name DragFullWindows -Value '0'; 'ok'`,
    revert: `Set-ItemProperty 'HKCU:\\Control Panel\\Desktop' -Name DragFullWindows -Value '1'; 'ok'`,
  },
  {
    id: 'font-smoothing-off',
    title: 'Reducir suavizado de fuentes (renderizado de texto)',
    desc: 'Desactiva el suavizado ClearType del texto. Gana algo de rendimiento; el texto se vera menos suave.',
    category: 'Animaciones',
    levels: ['agresiva'],
    risk: 'medium',
    needsAdmin: false,
    check: `$v = (Get-ItemProperty 'HKCU:\\Control Panel\\Desktop' -Name FontSmoothing -ErrorAction SilentlyContinue).FontSmoothing; if ("$v" -eq '0') { 'on' } else { 'off' }`,
    apply: `Set-ItemProperty 'HKCU:\\Control Panel\\Desktop' -Name FontSmoothing -Value '0'; 'ok'`,
    revert: `Set-ItemProperty 'HKCU:\\Control Panel\\Desktop' -Name FontSmoothing -Value '2'; 'ok'`,
  },

  // -------------------- LATENCIA --------------------
  {
    id: 'system-responsiveness',
    title: 'Latencia multimedia al minimo',
    desc: 'Pone SystemResponsiveness a 0 para que juegos y audio tengan maxima prioridad de CPU.',
    category: 'Latencia',
    levels: ['media', 'agresiva'],
    risk: 'medium',
    needsAdmin: true,
    check: `$v = (Get-ItemProperty 'HKLM:\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Multimedia\\SystemProfile' -Name SystemResponsiveness -ErrorAction SilentlyContinue).SystemResponsiveness; if ($v -eq 0) { 'on' } else { 'off' }`,
    apply: `Set-ItemProperty 'HKLM:\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Multimedia\\SystemProfile' -Name SystemResponsiveness -Value 0 -Type DWord; 'ok'`,
    revert: `Set-ItemProperty 'HKLM:\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Multimedia\\SystemProfile' -Name SystemResponsiveness -Value 20 -Type DWord; 'ok'`,
  },
  {
    id: 'games-priority',
    title: 'Prioridad alta para juegos (MMCSS)',
    desc: 'Sube la prioridad de CPU/GPU de la tarea "Games" del programador multimedia de Windows.',
    category: 'Latencia',
    levels: ['media', 'agresiva'],
    risk: 'medium',
    needsAdmin: true,
    check: `$p = 'HKLM:\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Multimedia\\SystemProfile\\Tasks\\Games'; $g = Get-ItemProperty $p -ErrorAction SilentlyContinue; if ($g.Priority -eq 6 -and "$($g.'Scheduling Category')" -eq 'High') { 'on' } else { 'off' }`,
    apply: `
      $p = 'HKLM:\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Multimedia\\SystemProfile\\Tasks\\Games'
      New-Item $p -Force | Out-Null
      Set-ItemProperty $p -Name 'GPU Priority' -Value 8 -Type DWord
      Set-ItemProperty $p -Name 'Priority' -Value 6 -Type DWord
      Set-ItemProperty $p -Name 'Scheduling Category' -Value 'High' -Type String
      Set-ItemProperty $p -Name 'SFIO Priority' -Value 'High' -Type String
      'ok'`,
    revert: `
      $p = 'HKLM:\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Multimedia\\SystemProfile\\Tasks\\Games'
      Set-ItemProperty $p -Name 'GPU Priority' -Value 8 -Type DWord
      Set-ItemProperty $p -Name 'Priority' -Value 2 -Type DWord
      Set-ItemProperty $p -Name 'Scheduling Category' -Value 'Medium' -Type String
      Set-ItemProperty $p -Name 'SFIO Priority' -Value 'Normal' -Type String
      'ok'`,
  },
  {
    id: 'nagle-off',
    title: 'Desactivar algoritmo de Nagle (red)',
    desc: 'Reduce la latencia en juegos online fijando TcpAckFrequency=1 en los adaptadores con IP activa.',
    category: 'Latencia',
    levels: ['agresiva'],
    risk: 'medium',
    needsAdmin: true,
    check: `
      $base = 'HKLM:\\SYSTEM\\CurrentControlSet\\Services\\Tcpip\\Parameters\\Interfaces'
      $hit = $false
      Get-ChildItem $base | ForEach-Object { if ((Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue).TcpAckFrequency -eq 1) { $hit = $true } }
      if ($hit) { 'on' } else { 'off' }`,
    apply: `
      $base = 'HKLM:\\SYSTEM\\CurrentControlSet\\Services\\Tcpip\\Parameters\\Interfaces'
      Get-ChildItem $base | ForEach-Object {
        $pp = $_.PSPath
        $prop = Get-ItemProperty $pp -ErrorAction SilentlyContinue
        $ip = $prop.IPAddress; $dh = $prop.DhcpIPAddress
        if (($dh) -or ($ip -and $ip -ne '0.0.0.0')) {
          Set-ItemProperty $pp -Name TcpAckFrequency -Value 1 -Type DWord
          Set-ItemProperty $pp -Name TCPNoDelay -Value 1 -Type DWord
        }
      }
      'ok'`,
    revert: `
      $base = 'HKLM:\\SYSTEM\\CurrentControlSet\\Services\\Tcpip\\Parameters\\Interfaces'
      Get-ChildItem $base | ForEach-Object {
        Remove-ItemProperty $_.PSPath -Name TcpAckFrequency -ErrorAction SilentlyContinue
        Remove-ItemProperty $_.PSPath -Name TCPNoDelay -ErrorAction SilentlyContinue
      }
      'ok'`,
  },

  // -------------------- RED (ancho de banda) --------------------
  {
    id: 'qos-bandwidth',
    title: 'Liberar ancho de banda reservado (QoS)',
    desc: 'Elimina el 20% de ancho de banda que Windows reserva por defecto para QoS, dejandolo disponible.',
    category: 'Red',
    levels: ['media', 'agresiva'],
    risk: 'medium',
    needsAdmin: true,
    check: `$v = (Get-ItemProperty 'HKLM:\\SOFTWARE\\Policies\\Microsoft\\Windows\\Psched' -Name NonBestEffortLimit -ErrorAction SilentlyContinue).NonBestEffortLimit; if ($v -eq 0) { 'on' } else { 'off' }`,
    apply: `
      New-Item 'HKLM:\\SOFTWARE\\Policies\\Microsoft\\Windows\\Psched' -Force | Out-Null
      Set-ItemProperty 'HKLM:\\SOFTWARE\\Policies\\Microsoft\\Windows\\Psched' -Name NonBestEffortLimit -Value 0 -Type DWord
      'ok'`,
    revert: `Remove-ItemProperty 'HKLM:\\SOFTWARE\\Policies\\Microsoft\\Windows\\Psched' -Name NonBestEffortLimit -ErrorAction SilentlyContinue; 'ok'`,
  },
  {
    id: 'tcp-throughput',
    title: 'TCP a maxima velocidad (autotuning + RSS)',
    desc: 'Activa el autoajuste de ventana TCP y Receive-Side Scaling para aprovechar todo el ancho de banda en descargas.',
    category: 'Red',
    levels: ['agresiva'],
    risk: 'medium',
    needsAdmin: true,
    check: `$a = (Get-NetTCPSetting -SettingName Internet -ErrorAction SilentlyContinue).AutoTuningLevelLocal; if ("$a" -eq 'Normal') { 'on' } else { 'off' }`,
    apply: `
      Set-NetTCPSetting -SettingName Internet -AutoTuningLevelLocal Normal -ErrorAction SilentlyContinue
      netsh int tcp set global rss=enabled | Out-Null
      netsh int tcp set global autotuninglevel=normal | Out-Null
      'ok'`,
    revert: `
      Set-NetTCPSetting -SettingName Internet -AutoTuningLevelLocal Normal -ErrorAction SilentlyContinue
      'ok'`,
  },

  // -------------------- GPU (ultra rendimiento) --------------------
  {
    id: 'nvidia-max-perf',
    title: 'NVIDIA: power limit al maximo (ultra)',
    desc: 'Sube el limite de potencia de la GPU NVIDIA a su maximo permitido via nvidia-smi para sostener frecuencias altas.',
    category: 'GPU',
    levels: ['agresiva'],
    risk: 'medium',
    needsAdmin: true,
    nvidiaOnly: true,
    check: `
      $smi = Get-Command nvidia-smi -ErrorAction SilentlyContinue
      if (-not $smi) { 'unknown' }
      else {
        $cur = (nvidia-smi --query-gpu=power.limit --format=csv,noheader,nounits) -replace '[^\\d\\.]',''
        $max = (nvidia-smi --query-gpu=power.max_limit --format=csv,noheader,nounits) -replace '[^\\d\\.]',''
        if ($cur -and $max -and ([double]$cur -ge ([double]$max - 0.5))) { 'on' } else { 'off' }
      }`,
    apply: `
      $max = (nvidia-smi --query-gpu=power.max_limit --format=csv,noheader,nounits) -replace '[^\\d\\.]',''
      if ($max) { nvidia-smi -pl $max | Out-Null }
      'ok'`,
    revert: `
      $def = (nvidia-smi --query-gpu=power.default_limit --format=csv,noheader,nounits) -replace '[^\\d\\.]',''
      if ($def) { nvidia-smi -pl $def | Out-Null }
      'ok'`,
  },
  {
    id: 'amd-max-perf',
    title: 'AMD: abrir Adrenalin (perfil rendimiento)',
    desc: 'Abre AMD Software para activar el perfil de maximo rendimiento (no es scriptable de forma segura).',
    category: 'GPU',
    levels: ['agresiva'],
    risk: 'low',
    needsAdmin: false,
    amdOnly: true,
    informational: true,
    irreversible: true,
    check: `'unknown'`,
    apply: `
      $exe = Join-Path $env:ProgramFiles 'AMD\\CNext\\CNext\\RadeonSoftware.exe'
      if (Test-Path $exe) { Start-Process $exe } else { Start-Process 'https://www.amd.com/en/products/software-adrenalin-edition.html' }
      'ok'`,
    revert: `'no-aplica'`,
  },

  // -------------------- HERRAMIENTAS (instalacion) --------------------
  {
    id: 'msi-center-install',
    title: 'Instalar MSI Center (perfiles de ventiladores)',
    desc: 'Instala MSI Center via winget para activar perfiles de maximo rendimiento de ventiladores en equipos MSI.',
    category: 'Herramientas',
    levels: ['media', 'agresiva'],
    risk: 'low',
    needsAdmin: true,
    isInstall: true,
    timeout: 360000,
    check: `$o = winget list -e --id MSI.MSICenter --accept-source-agreements --disable-interactivity 2>$null | Out-String; if ($o -match 'MSI\\.MSICenter') { 'on' } else { 'off' }`,
    apply: `winget install -e --id MSI.MSICenter --accept-source-agreements --accept-package-agreements --disable-interactivity --silent | Out-Null; 'ok'`,
    revert: `winget uninstall -e --id MSI.MSICenter --disable-interactivity --silent | Out-Null; 'ok'`,
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
