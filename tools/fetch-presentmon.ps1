# Downloads the pinned PresentMon console build (MIT, Intel/GameTechDev) and verifies it.
# The installer bundles it as presentmon\PresentMon.exe next to overdrive-svc.exe.
$ErrorActionPreference = 'Stop'
$version = '2.6.0'
$sha256  = 'b2a706bc6ad475749e3b7e3409263aa1e6906d45bdcf993f6dbc0f660188f1af'
$url     = "https://github.com/GameTechDev/PresentMon/releases/download/v$version/PresentMon-$version-x64.exe"
$dir     = Join-Path $PSScriptRoot 'presentmon'
$out     = Join-Path $dir 'PresentMon.exe'
New-Item -ItemType Directory -Force $dir | Out-Null
if ((Test-Path $out) -and ((Get-FileHash $out -Algorithm SHA256).Hash.ToLower() -eq $sha256)) { "PresentMon $version already present"; exit 0 }
Invoke-WebRequest -Uri $url -OutFile "$out.download" -UseBasicParsing
$got = (Get-FileHash "$out.download" -Algorithm SHA256).Hash.ToLower()
if ($got -ne $sha256) { Remove-Item "$out.download"; throw "PresentMon hash mismatch: $got" }
Move-Item -Force "$out.download" $out
"PresentMon $version OK ($out)"
