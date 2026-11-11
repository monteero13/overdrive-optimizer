'use strict';
// Analisis del equipo: hardware, software, estado de energia y drivers.
// Todo se obtiene via PowerShell/CIM. No modifica nada (solo lectura).
//
// IMPORTANTE: el escaneo de drivers (Win32_PnPSignedDriver) puede ser MUY lento
// en algunos equipos, asi que va en una funcion aparte (analyzeDrivers) que el
// renderer llama de forma no bloqueante. El hardware se obtiene rapido y el plan
// puede mostrarse sin esperar a los drivers.

const { psRun } = require('./ps');

const HELPERS = String.raw`
$ErrorActionPreference = 'SilentlyContinue'
function Is-Admin {
  $id = [Security.Principal.WindowsIdentity]::GetCurrent()
  (New-Object Security.Principal.WindowsPrincipal($id)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}
function To-Date($val, $fmt) {
  if (-not $val) { return $null }
  try {
    if ($val -is [datetime]) { return $val.ToString($fmt) }
    return [Management.ManagementDateTimeConverter]::ToDateTime([string]$val).ToString($fmt)
  } catch { return $null }
}
`;

// ---------------------- HARDWARE (rapido) ----------------------
const HARDWARE_SCRIPT = HELPERS + String.raw`
$os  = Get-CimInstance Win32_OperatingSystem
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$gpus = Get-CimInstance Win32_VideoController
$mem = Get-CimInstance Win32_PhysicalMemory
$disks = Get-CimInstance Win32_DiskDrive
$vols = Get-CimInstance Win32_LogicalDisk -Filter "DriveType=3"

$totalRamGB = [math]::Round(($os.TotalVisibleMemorySize * 1KB) / 1GB, 1)
$freeRamGB  = [math]::Round(($os.FreePhysicalMemory * 1KB) / 1GB, 1)

$ramModules = @()
foreach ($m in $mem) {
  $ramModules += [ordered]@{ sizeGB = [math]::Round($m.Capacity / 1GB, 0); speed = [int]$m.Speed; slot = "$($m.DeviceLocator)" }
}

$gpuList = @()
foreach ($g in $gpus) {
  $dd = To-Date $g.DriverDate 'yyyy-MM-dd'
  $gpuList += [ordered]@{
    name = "$($g.Name)"
    driverVersion = "$($g.DriverVersion)"
    driverDate = $dd
    vramGB = if ($g.AdapterRAM -gt 0) { [math]::Round($g.AdapterRAM / 1GB, 1) } else { $null }
  }
}

$diskList = @()
foreach ($d in $disks) {
  $media = 'Desconocido'
  try { $pd = Get-PhysicalDisk | Where-Object { $_.DeviceId -eq $d.Index }; if ($pd) { $media = "$($pd.MediaType)" } } catch {}
  $diskList += [ordered]@{ model = "$($d.Model)"; sizeGB = [math]::Round($d.Size / 1GB, 0); media = $media }
}

$volList = @()
foreach ($v in $vols) {
  $volList += [ordered]@{ drive = "$($v.DeviceID)"; totalGB = [math]::Round($v.Size / 1GB, 0); freeGB = [math]::Round($v.FreeSpace / 1GB, 0) }
}

$power = 'Desconocido'
try { $pp = powercfg /getactivescheme; if ($pp -match '\(([^)]+)\)') { $power = $Matches[1] } } catch {}

$top = Get-Process | Sort-Object WorkingSet64 -Descending | Select-Object -First 8 | ForEach-Object {
  [ordered]@{ name = $_.ProcessName; memMB = [math]::Round($_.WorkingSet64 / 1MB, 0) }
}

$lastBoot = To-Date $os.LastBootUpTime 'yyyy-MM-dd HH:mm'

$result = [ordered]@{
  admin = (Is-Admin)
  os = [ordered]@{ name = "$($os.Caption)"; version = "$($os.Version)"; build = "$($os.BuildNumber)"; arch = "$($os.OSArchitecture)"; lastBoot = $lastBoot }
  cpu = [ordered]@{ name = "$($cpu.Name)".Trim(); cores = [int]$cpu.NumberOfCores; threads = [int]$cpu.NumberOfLogicalProcessors; clockMHz = [int]$cpu.MaxClockSpeed; load = [int]$cpu.LoadPercentage }
  ram = [ordered]@{ totalGB = $totalRamGB; freeGB = $freeRamGB; modules = $ramModules }
  gpu = $gpuList
  disks = $diskList
  volumes = $volList
  powerPlan = $power
  topProcesses = $top
}
$result | ConvertTo-Json -Depth 8 -Compress
`;

// ---------------------- DRIVERS (lento, aparte) ----------------------
const DRIVERS_SCRIPT = HELPERS + String.raw`
$now = Get-Date
# Filtramos en la propia consulta WQL a las clases que importan para reducir coste.
$flt = "DeviceClass='DISPLAY' OR DeviceClass='NET' OR DeviceClass='MEDIA' OR DeviceClass='BLUETOOTH'"
$drvAll = Get-CimInstance Win32_PnPSignedDriver -Filter $flt | Where-Object { $_.DriverDate }
$drivers = @()
$scanned = 0
foreach ($dr in $drvAll) {
  $scanned++
  $dstr = To-Date $dr.DriverDate 'yyyy-MM-dd'
  if (-not $dstr) { continue }
  $age = [int]($now - [datetime]$dstr).TotalDays
  $drivers += [ordered]@{
    device = "$($dr.DeviceName)"; class = "$($dr.DeviceClass)"; version = "$($dr.DriverVersion)"
    date = $dstr; ageDays = $age; manufacturer = "$($dr.Manufacturer)"
  }
}
[ordered]@{ drivers = $drivers; driversScanned = $scanned } | ConvertTo-Json -Depth 6 -Compress
`;

// ---------------------- enriquecido de drivers (JS) ----------------------
function vendorUrlFor(driver) {
  const text = `${driver.device} ${driver.manufacturer}`.toLowerCase();
  const cls = (driver.class || '').toUpperCase();
  if (cls === 'DISPLAY') {
    if (text.includes('nvidia')) return 'https://www.nvidia.com/Download/index.aspx';
    if (text.includes('amd') || text.includes('radeon')) return 'https://www.amd.com/support';
    if (text.includes('intel')) return 'https://www.intel.com/content/www/us/en/download-center/home.html';
  }
  if (cls === 'NET') {
    if (text.includes('intel')) return 'https://www.intel.com/content/www/us/en/download-center/home.html';
    if (text.includes('realtek')) return 'https://www.realtek.com/Download';
  }
  if (cls === 'MEDIA' && text.includes('realtek')) return 'https://www.realtek.com/Download';
  return 'ms-settings:windowsupdate';
}

function severityFor(driver) {
  if (driver.ageDays >= 7300) return 'ok'; // fecha centinela de driver inbox
  const cls = (driver.class || '').toUpperCase();
  const critical = cls === 'DISPLAY' || cls === 'NET';
  if (driver.ageDays >= 1095) return 'muy-desactualizado';
  if (driver.ageDays >= (critical ? 540 : 900)) return 'desactualizado';
  return 'ok';
}

// ---------------------- API ----------------------
async function analyzeHardware() {
  const raw = (await psRun(HARDWARE_SCRIPT, { timeout: 60000 })).trim();
  if (!raw) throw new Error('El analisis no devolvio datos.');
  const data = JSON.parse(raw);
  for (const key of ['gpu', 'disks', 'volumes', 'topProcesses']) {
    if (data[key] && !Array.isArray(data[key])) data[key] = [data[key]];
    if (!data[key]) data[key] = [];
  }
  if (data.ram && data.ram.modules && !Array.isArray(data.ram.modules)) data.ram.modules = [data.ram.modules];
  return data;
}

async function analyzeDrivers() {
  const raw = (await psRun(DRIVERS_SCRIPT, { timeout: 120000 })).trim();
  if (!raw) return { driversScanned: 0, driversOutdated: [] };
  const data = JSON.parse(raw);
  const all = Array.isArray(data.drivers) ? data.drivers : (data.drivers ? [data.drivers] : []);
  const seen = new Set();
  const flagged = all
    .map((d) => ({ ...d, severity: severityFor(d), vendorUrl: vendorUrlFor(d) }))
    .filter((d) => d.severity !== 'ok')
    .filter((d) => {
      const key = `${d.device}|${d.version}`;
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    })
    .sort((a, b) => b.ageDays - a.ageDays);
  return { driversScanned: data.driversScanned || 0, driversOutdated: flagged };
}

module.exports = { analyzeHardware, analyzeDrivers };
