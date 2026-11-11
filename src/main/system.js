'use strict';
// Analisis del equipo: hardware, software, estado de energia y drivers.
// Todo se obtiene via PowerShell/CIM. No modifica nada (solo lectura).

const { psRun } = require('./ps');

// Script unico que recoge todo el inventario y lo devuelve como un objeto JSON.
// Se calculan las edades de driver en PowerShell para evitar problemas de
// serializacion de fechas entre Windows PowerShell 5.1 y PowerShell 7.
const ANALYZE_SCRIPT = String.raw`
$ErrorActionPreference = 'SilentlyContinue'

function Is-Admin {
  $id = [Security.Principal.WindowsIdentity]::GetCurrent()
  (New-Object Security.Principal.WindowsPrincipal($id)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

# Conversion de fecha tolerante: acepta DateTime (lo normal via Get-CimInstance)
# o cadena DMTF, y devuelve $null si el valor es invalido.
function To-Date($val, $fmt) {
  if (-not $val) { return $null }
  try {
    if ($val -is [datetime]) { return $val.ToString($fmt) }
    return [Management.ManagementDateTimeConverter]::ToDateTime([string]$val).ToString($fmt)
  } catch { return $null }
}

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
  $ramModules += [ordered]@{
    sizeGB = [math]::Round($m.Capacity / 1GB, 0)
    speed  = [int]$m.Speed
    slot   = "$($m.DeviceLocator)"
  }
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
  try {
    $pd = Get-PhysicalDisk | Where-Object { $_.DeviceId -eq $d.Index }
    if ($pd) { $media = "$($pd.MediaType)" }
  } catch {}
  $diskList += [ordered]@{
    model = "$($d.Model)"
    sizeGB = [math]::Round($d.Size / 1GB, 0)
    media = $media
  }
}

$volList = @()
foreach ($v in $vols) {
  $volList += [ordered]@{
    drive = "$($v.DeviceID)"
    totalGB = [math]::Round($v.Size / 1GB, 0)
    freeGB  = [math]::Round($v.FreeSpace / 1GB, 0)
  }
}

# Plan de energia activo
$power = 'Desconocido'
try {
  $pp = powercfg /getactivescheme
  if ($pp -match '\(([^)]+)\)') { $power = $Matches[1] }
} catch {}

# Procesos que mas RAM consumen
$top = Get-Process | Sort-Object WorkingSet64 -Descending | Select-Object -First 8 | ForEach-Object {
  [ordered]@{ name = $_.ProcessName; memMB = [math]::Round($_.WorkingSet64 / 1MB, 0) }
}

# Drivers: clases relevantes para gaming. Calculamos la edad en dias.
# Clases que el usuario realmente actualiza y que impactan en gaming.
# Se excluyen chipset/USB/almacenamiento inbox (fechas centinela, los gestiona Windows).
$classes = @('Display','Net','Media','Bluetooth')
$now = Get-Date
$drvAll = Get-CimInstance Win32_PnPSignedDriver | Where-Object { $_.DeviceClass -and ($classes -contains $_.DeviceClass) -and $_.DriverDate }
$drivers = @()
$scanned = 0
foreach ($dr in $drvAll) {
  $scanned++
  $dstr = To-Date $dr.DriverDate 'yyyy-MM-dd'
  if (-not $dstr) { continue }
  $age = [int]($now - [datetime]$dstr).TotalDays
  $drivers += [ordered]@{
    device = "$($dr.DeviceName)"
    class  = "$($dr.DeviceClass)"
    version = "$($dr.DriverVersion)"
    date = $dstr
    ageDays = $age
    manufacturer = "$($dr.Manufacturer)"
  }
}

$lastBoot = To-Date $os.LastBootUpTime 'yyyy-MM-dd HH:mm'

$result = [ordered]@{
  admin = (Is-Admin)
  os = [ordered]@{
    name = "$($os.Caption)"
    version = "$($os.Version)"
    build = "$($os.BuildNumber)"
    arch = "$($os.OSArchitecture)"
    lastBoot = $lastBoot
  }
  cpu = [ordered]@{
    name = "$($cpu.Name)".Trim()
    cores = [int]$cpu.NumberOfCores
    threads = [int]$cpu.NumberOfLogicalProcessors
    clockMHz = [int]$cpu.MaxClockSpeed
    load = [int]$cpu.LoadPercentage
  }
  ram = [ordered]@{
    totalGB = $totalRamGB
    freeGB = $freeRamGB
    modules = $ramModules
  }
  gpu = $gpuList
  disks = $diskList
  volumes = $volList
  powerPlan = $power
  topProcesses = $top
  drivers = $drivers
  driversScanned = $scanned
}

$result | ConvertTo-Json -Depth 8 -Compress
`;

// Mapa clase/fabricante -> URL de descarga oficial recomendada.
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
  // Por defecto: Windows Update suele ser la via mas segura.
  return 'ms-settings:windowsupdate';
}

// Clasifica severidad segun antiguedad. GPU/Red se consideran mas criticos.
function severityFor(driver) {
  // Fechas centinela de drivers inbox (p.ej. 1968): no son actualizables, se ignoran.
  if (driver.ageDays >= 7300) return 'ok'; // > 20 años = sentinel
  const cls = (driver.class || '').toUpperCase();
  const critical = cls === 'DISPLAY' || cls === 'NET';
  if (driver.ageDays >= 1095) return 'muy-desactualizado'; // > 3 años
  if (driver.ageDays >= (critical ? 540 : 900)) return 'desactualizado';
  return 'ok';
}

async function analyze() {
  const raw = (await psRun(ANALYZE_SCRIPT, { timeout: 180000 })).trim();
  if (!raw) throw new Error('El analisis no devolvio datos.');
  const data = JSON.parse(raw);

  // Enriquecer drivers con severidad + url, y quedarnos solo con los relevantes.
  const allDrivers = Array.isArray(data.drivers) ? data.drivers : (data.drivers ? [data.drivers] : []);
  const seen = new Set();
  const flagged = allDrivers
    .map((d) => ({ ...d, severity: severityFor(d), vendorUrl: vendorUrlFor(d) }))
    .filter((d) => d.severity !== 'ok')
    .filter((d) => {
      const key = `${d.device}|${d.version}`;
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    })
    .sort((a, b) => b.ageDays - a.ageDays);

  data.driversOutdated = flagged;
  // Normalizar arrays que CIM puede devolver como objeto suelto cuando hay 1 elemento.
  for (const key of ['gpu', 'disks', 'volumes', 'topProcesses']) {
    if (data[key] && !Array.isArray(data[key])) data[key] = [data[key]];
    if (!data[key]) data[key] = [];
  }
  if (data.ram && data.ram.modules && !Array.isArray(data.ram.modules)) {
    data.ram.modules = [data.ram.modules];
  }
  return data;
}

module.exports = { analyze };
