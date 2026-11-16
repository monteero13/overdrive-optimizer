# Footprint check (goals.md Â§4.3): the service while idle (no game running).
#   Budget: < 15 MB private working set, < 0.1 % of one core.
#
#   powershell -File tools\footprint.ps1 [-Exe target\release\overdrive-svc.exe] [-Seconds 60]
#
# Runs `overdrive-svc console` in DRY-RUN mode (real reads, simulated writes: nothing on
# the PC changes) on its own pipe and data folder, waits for it to settle, then measures.
# Exit code 1 if a budget is exceeded. Used by .github/workflows/ci.yml.
param(
    [string]$Exe = "target\release\overdrive-svc.exe",
    [int]$WarmupSeconds = 30,
    [int]$Seconds = 60,
    [double]$MaxPrivateMB = 15,
    [double]$MaxCpuPct = 0.1
)
$ErrorActionPreference = 'Stop'
if (-not (Test-Path $Exe)) { throw "not found: $Exe (cargo build --release -p overdrive-svc)" }

$data = Join-Path ([IO.Path]::GetTempPath()) ("overdrive-footprint-" + [guid]::NewGuid())
$pipe = "\\.\pipe\overdrive.footprint." + $PID
$log = Join-Path ([IO.Path]::GetTempPath()) ("overdrive-footprint-" + $PID + ".log")
$p = Start-Process -FilePath $Exe -ArgumentList @('console', '--data', $data, '--pipe', $pipe) `
    -PassThru -WindowStyle Hidden -RedirectStandardError $log
try {
    Start-Sleep -Seconds $WarmupSeconds
    if ($p.HasExited) { Get-Content $log; throw "the service exited during warm-up (code $($p.ExitCode))" }

    # Private working set = what this process alone keeps in RAM (Task Manager's "Memory").
    $privateWs = {
        (Get-CimInstance Win32_PerfRawData_PerfProc_Process -Filter "IDProcess = $($p.Id)").WorkingSetPrivate
    }
    $p.Refresh()
    $cpu0 = $p.TotalProcessorTime.TotalMilliseconds
    $t0 = [Diagnostics.Stopwatch]::StartNew()
    $peak = 0
    # Sample memory every 10 s: probing more often (WMI) adds activity the service sees.
    for ($i = 0; $i -lt $Seconds; $i += 10) {
        Start-Sleep -Seconds 10
        $ws = & $privateWs
        if ($ws -gt $peak) { $peak = $ws }
    }
    $p.Refresh()
    $cpuMs = $p.TotalProcessorTime.TotalMilliseconds - $cpu0
    $wallMs = $t0.Elapsed.TotalMilliseconds
    $cpuPct = 100.0 * $cpuMs / $wallMs
    $peakMB = $peak / 1MB

    $result = [ordered]@{
        privateWorkingSetMB = [math]::Round($peakMB, 2)
        budgetMB            = $MaxPrivateMB
        cpuPctOfOneCore     = [math]::Round($cpuPct, 4)
        budgetCpuPct        = $MaxCpuPct
        sampledSeconds      = $Seconds
    }
    $result | ConvertTo-Json | Write-Output
    $ok = ($peakMB -lt $MaxPrivateMB) -and ($cpuPct -lt $MaxCpuPct)
    if (-not $ok) { Write-Output "FOOTPRINT BUDGET EXCEEDED"; exit 1 }
    Write-Output "footprint within budget"
}
finally {
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
    Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
    Remove-Item -Force $log -ErrorAction SilentlyContinue
}
