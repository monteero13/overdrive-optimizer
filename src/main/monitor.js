'use strict';
// Muestreo de métricas en tiempo real (solo lectura):
//   GPU  -> nvidia-smi (temp, uso, reloj, vatios, ventilador)
//   CPU  -> WMI (uso, reloj) + zona térmica ACPI (aprox., best-effort)
//   RAM  -> WMI (uso / total)

const { execFile } = require('child_process');
const { psRun } = require('./ps');

// nvidia-smi directo (más rápido que pasar por PowerShell).
function nvidiaQuery() {
  return new Promise((resolve) => {
    execFile(
      'nvidia-smi',
      [
        '--query-gpu=temperature.gpu,utilization.gpu,utilization.memory,clocks.current.graphics,power.draw,power.limit,fan.speed',
        '--format=csv,noheader,nounits',
      ],
      { windowsHide: true, timeout: 8000 },
      (err, stdout) => {
        if (err || !stdout) return resolve(null);
        const line = stdout.trim().split(/\r?\n/)[0];
        if (!line) return resolve(null);
        const p = line.split(',').map((s) => parseFloat(String(s).replace(/[^\d.\-]/g, '')));
        const num = (v) => (Number.isFinite(v) ? v : null);
        resolve({
          tempC: num(p[0]),
          util: num(p[1]),
          memUtil: num(p[2]),
          clockMHz: num(p[3]),
          powerW: num(p[4]),
          powerLimitW: num(p[5]),
          fanPct: num(p[6]),
        });
      }
    );
  });
}

const CPU_SCRIPT = String.raw`
$ErrorActionPreference = 'SilentlyContinue'
$c = Get-CimInstance Win32_Processor
$load = [int](($c | Measure-Object -Property LoadPercentage -Average).Average)
$clock = [int](($c | Measure-Object -Property CurrentClockSpeed -Average).Average)
$os = Get-CimInstance Win32_OperatingSystem
$tot = [math]::Round(($os.TotalVisibleMemorySize * 1KB) / 1GB, 1)
$free = [math]::Round(($os.FreePhysicalMemory * 1KB) / 1GB, 1)
$temp = $null
try {
  $t = Get-CimInstance -Namespace root/wmi -ClassName MSAcpi_ThermalZoneTemperature -ErrorAction Stop | Select-Object -First 1
  if ($t) { $temp = [math]::Round(($t.CurrentTemperature / 10) - 273.15, 1) }
} catch {}
[ordered]@{
  cpuLoad = $load
  cpuClock = $clock
  cpuTemp = $temp
  ramUsedGB = [math]::Round($tot - $free, 1)
  ramTotalGB = $tot
} | ConvertTo-Json -Compress
`;

async function cpuRamQuery() {
  try {
    const out = (await psRun(CPU_SCRIPT, { timeout: 8000 })).trim();
    return out ? JSON.parse(out) : null;
  } catch (_) {
    return null;
  }
}

async function sampleMetrics() {
  const [gpu, sys] = await Promise.all([nvidiaQuery(), cpuRamQuery()]);
  return {
    t: Date.now(),
    gpu,
    cpu: sys ? { load: sys.cpuLoad, clock: sys.cpuClock, temp: sys.cpuTemp } : null,
    ram: sys ? { usedGB: sys.ramUsedGB, totalGB: sys.ramTotalGB } : null,
  };
}

module.exports = { sampleMetrics };
