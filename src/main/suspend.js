'use strict';
// "Aligerar" apps de fondo durante la partida SIN congelarlas.
//
// Antes se usaba NtSuspendProcess: congelaba las apps (ventanas "No responde",
// juegos colgados al hablar con el overlay de Discord/Steam) y NO liberaba RAM
// (un proceso suspendido conserva toda su memoria). Ahora:
//   1. EmptyWorkingSet  -> saca de la RAM las páginas que la app no está usando.
//                          Libera memoria real y la app sigue funcionando; si la
//                          necesita, Windows la vuelve a cargar al momento.
//   2. Prioridad BelowNormal -> cede CPU al juego (excepto apps acopladas al juego).
// El estado (prioridad original por PID) se guarda en disco para restaurarlo
// al desactivar, al cerrar Overdrive o, si se cerró mal, al volver a abrirlo.

const fs = require('fs');
const os = require('os');
const path = require('path');
const { psRun } = require('./ps');

// Apps con overlay/IPC dentro del juego o de las que depende el juego/launcher:
// se recorta su RAM pero NO se les baja la prioridad (voz de Discord, overlay de Steam, EOS de Epic).
const NO_DEPRIO = ['discord', 'steamwebhelper', 'epicgameslauncher'];

let stateFile = path.join(os.tmpdir(), 'overdrive-background-state.json');
function init(dir) {
  stateFile = path.join(dir, 'background-state.json');
}

// { [pid]: { name, start (FILETIME como string: no cabe en un Number), orig, owners: [] } }
function loadState() {
  try {
    return JSON.parse(fs.readFileSync(stateFile, 'utf8')) || {};
  } catch (_) {
    return {};
  }
}
function saveState(s) {
  try {
    fs.writeFileSync(stateFile, JSON.stringify(s, null, 2));
  } catch (_) {
    /* noop */
  }
}

// Serializa todas las operaciones (botón Modo Juego, vigilante de perfiles, cierre).
let chain = Promise.resolve();
function serial(fn) {
  const run = chain.then(fn, fn);
  chain = run.catch(() => {});
  return run;
}

const psList = (arr) => '@(' + arr.map((n) => `'${String(n).replace(/'/g, "''")}'`).join(',') + ')';

const TYPES = String.raw`
$ErrorActionPreference='SilentlyContinue'
$sig = @'
using System;
using System.Runtime.InteropServices;
public static class OvdBg {
  [DllImport("psapi.dll")] public static extern bool EmptyWorkingSet(IntPtr h);
  [DllImport("ntdll.dll")] public static extern uint NtResumeProcess(IntPtr h);
}
'@
Add-Type -TypeDefinition $sig -ErrorAction SilentlyContinue
function AvailMB { [math]::Round((Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory / 1KB) }
`;

async function runJson(script, timeout = 30000) {
  const out = (await psRun(script, { timeout })).trim();
  return out ? JSON.parse(out) : null;
}

/**
 * Recorta RAM y (si procede) baja prioridad de los procesos con esos nombres.
 * @param {string[]} names  nombres de proceso sin .exe
 * @param {string} owner    quién lo pide ('gamemode' | 'profile:<id>')
 */
function suspend(names, owner = 'gamemode') {
  return serial(async () => {
    if (!names || !names.length) return { freedMB: 0, count: 0 };
    const state = loadState();
    const script =
      TYPES +
      `
      $names = ${psList(names)}
      $noDeprio = ${psList(NO_DEPRIO)}
      $before = AvailMB
      $items = @()
      foreach ($n in $names) {
        Get-Process -Name $n -ErrorAction SilentlyContinue | ForEach-Object {
          $p = $_
          try {
            $orig = "$($p.PriorityClass)"
            [OvdBg]::EmptyWorkingSet($p.Handle) | Out-Null
            $lowered = $false
            if (($noDeprio -notcontains $p.ProcessName.ToLower()) -and ($orig -in @('Normal','AboveNormal'))) {
              $p.PriorityClass = 'BelowNormal'; $lowered = $true
            }
            $items += [ordered]@{ pid = $p.Id; name = $p.ProcessName; start = "$($p.StartTime.ToFileTimeUtc())"; orig = $orig; lowered = $lowered }
          } catch {}
        }
      }
      Start-Sleep -Milliseconds 1500
      $after = AvailMB
      [ordered]@{ freedMB = [math]::Max(0, $after - $before); count = $items.Count; items = @($items) } | ConvertTo-Json -Compress -Depth 4`;
    let r;
    try {
      r = (await runJson(script)) || { freedMB: 0, count: 0, items: [] };
    } catch (e) {
      return { freedMB: 0, count: 0, error: e.message };
    }
    for (const it of r.items || []) {
      const key = String(it.pid);
      const prev = state[key];
      if (prev && prev.start === it.start) {
        // Ya lo gestionábamos: conservar la prioridad ORIGINAL, solo añadir el dueño.
        if (!prev.owners.includes(owner)) prev.owners.push(owner);
        prev.lowered = prev.lowered || it.lowered;
      } else {
        state[key] = { name: it.name, start: it.start, orig: it.orig, lowered: it.lowered, owners: [owner] };
      }
    }
    saveState(state);
    // Contar apps (no procesos): Discord tiene ~6 procesos y es 1 app.
    const apps = new Set((r.items || []).map((i) => String(i.name).toLowerCase())).size;
    return { freedMB: r.freedMB || 0, count: apps, processes: r.count || 0 };
  });
}

async function restorePriorities(entries) {
  const todo = entries.filter((e) => e.lowered);
  if (!todo.length) return 0;
  const list = todo.map((e) => `@{pid=${Number(e.pid)};start='${String(e.start).replace(/\D/g, '')}';orig='${String(e.orig).replace(/'/g, "''")}'}`).join(',');
  const script = `
    $ErrorActionPreference='SilentlyContinue'
    $c = 0
    foreach ($e in @(${list})) {
      $p = Get-Process -Id $e.pid -ErrorAction SilentlyContinue
      # Comprobar StartTime para no tocar otro proceso que haya reutilizado el PID.
      if ($p -and "$($p.StartTime.ToFileTimeUtc())" -eq $e.start) {
        try { $p.PriorityClass = $e.orig; $c++ } catch {}
      }
    }
    $c`;
  try {
    return Number((await psRun(script, { timeout: 30000 })).trim()) || 0;
  } catch (_) {
    return 0;
  }
}

/** Quita el dueño y restaura la prioridad de los procesos que ya no tienen dueños. */
function resume(_names, owner = 'gamemode') {
  return serial(async () => {
    const state = loadState();
    const release = [];
    for (const [pid, e] of Object.entries(state)) {
      e.owners = (e.owners || []).filter((o) => o !== owner);
      if (!e.owners.length) {
        release.push({ pid, ...e });
        delete state[pid];
      }
    }
    const count = await restorePriorities(release);
    saveState(state);
    return { count };
  });
}

/** Restaura todo (al cerrar la app o al arrancar tras un cierre inesperado). */
function restoreAll() {
  return serial(async () => {
    const state = loadState();
    const entries = Object.entries(state).map(([pid, e]) => ({ pid, ...e }));
    const count = await restorePriorities(entries);
    saveState({});
    return { count };
  });
}

function isActive(owner = 'gamemode') {
  return Object.values(loadState()).some((e) => (e.owners || []).includes(owner));
}

/**
 * Rescate: reanuda procesos que versiones anteriores de Overdrive dejaron
 * congelados con NtSuspendProcess (p.ej. si la app se cerró en Modo Juego).
 * Solo actúa sobre los nombres dados y solo si TODOS sus hilos están suspendidos.
 */
function rescueFrozen(names) {
  return serial(async () => {
    if (!names || !names.length) return { count: 0 };
    const script =
      TYPES +
      `
      $c = 0
      foreach ($n in ${psList(names)}) {
        Get-Process -Name $n -ErrorAction SilentlyContinue | ForEach-Object {
          $p = $_; $tries = 0
          while ($tries -lt 16) {
            $p.Refresh(); $t = $p.Threads
            if ($t.Count -eq 0 -or @($t | Where-Object { $_.WaitReason -eq 'Suspended' }).Count -lt $t.Count) { break }
            [OvdBg]::NtResumeProcess($p.Handle) | Out-Null; $tries++
          }
          if ($tries -gt 0) { $c++ }
        }
      }
      $c`;
    try {
      return { count: Number((await psRun(script, { timeout: 30000 })).trim()) || 0 };
    } catch (e) {
      return { count: 0, error: e.message };
    }
  });
}

module.exports = { init, suspend, resume, restoreAll, rescueFrozen, isActive, NO_DEPRIO };
