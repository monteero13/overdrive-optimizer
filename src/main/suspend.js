'use strict';
// Suspende/reanuda procesos de fondo (no los mata) usando NtSuspendProcess /
// NtResumeProcess de ntdll via PowerShell Add-Type. Devuelve la RAM "liberada"
// (working set de los procesos suspendidos).

const { psRun } = require('./ps');

const ADDTYPE = String.raw`
$ErrorActionPreference='SilentlyContinue'
$sig = @'
using System;
using System.Runtime.InteropServices;
public static class Ovd {
  [DllImport("ntdll.dll")] public static extern uint NtSuspendProcess(IntPtr h);
  [DllImport("ntdll.dll")] public static extern uint NtResumeProcess(IntPtr h);
}
'@
Add-Type -TypeDefinition $sig -ErrorAction SilentlyContinue
`;

function nameList(names) {
  return '@(' + names.map((n) => `'${String(n).replace(/'/g, "''")}'`).join(',') + ')';
}

async function suspend(names) {
  if (!names || !names.length) return { freedMB: 0, count: 0 };
  const script =
    ADDTYPE +
    `
    $names = ${nameList(names)}
    $freed = 0; $c = 0
    foreach ($n in $names) {
      Get-Process -Name $n -ErrorAction SilentlyContinue | ForEach-Object {
        try { [Ovd]::NtSuspendProcess($_.Handle) | Out-Null; $freed += $_.WorkingSet64; $c++ } catch {}
      }
    }
    [ordered]@{ freedMB = [math]::Round($freed/1MB,0); count = $c } | ConvertTo-Json -Compress`;
  try {
    const out = (await psRun(script, { timeout: 20000 })).trim();
    return out ? JSON.parse(out) : { freedMB: 0, count: 0 };
  } catch (e) {
    return { freedMB: 0, count: 0, error: e.message };
  }
}

async function resume(names) {
  if (!names || !names.length) return { count: 0 };
  const script =
    ADDTYPE +
    `
    $names = ${nameList(names)}
    $c = 0
    foreach ($n in $names) {
      Get-Process -Name $n -ErrorAction SilentlyContinue | ForEach-Object {
        try { [Ovd]::NtResumeProcess($_.Handle) | Out-Null; $c++ } catch {}
      }
    }
    [ordered]@{ count = $c } | ConvertTo-Json -Compress`;
  try {
    const out = (await psRun(script, { timeout: 20000 })).trim();
    return out ? JSON.parse(out) : { count: 0 };
  } catch (e) {
    return { count: 0, error: e.message };
  }
}

module.exports = { suspend, resume };
