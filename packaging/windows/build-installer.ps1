# Builds the Inno Setup installer vivi-<Version>-windows-<Arch>-setup.exe.
#   pwsh packaging/windows/build-installer.ps1 -Exe target\release\vivi.exe -Mpv mpv\libmpv-2.dll `
#        -Version 0.16.6 -Arch x64 -OutDir dist
# Stages vivi.exe, libmpv-2.dll, the VC++ runtime DLLs vivi.exe imports (copied app-local from
# the Visual Studio redist folder, so no separate VC++ Redistributable is needed), the icon and
# the docs, then runs ISCC (Inno Setup 6; installed with Chocolatey when missing).
param(
  [Parameter(Mandatory)] [string] $Exe,
  [Parameter(Mandatory)] [string] $Mpv,
  [string] $Version = '',
  [ValidateSet('x64', 'arm64')] [string] $Arch = 'x64',
  [string] $OutDir = 'dist'
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path "$PSScriptRoot\..\..").Path
$Exe = (Resolve-Path $Exe).Path
$Mpv = (Resolve-Path $Mpv).Path
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
# Inno AppVersion / VersionInfoVersion must be numeric x.y.z or x.y.z.w.
# "0.16.4-dev.abc1234" uses 0.16.4 there; the full string stays in NameVersion (filename, AppVerName).
if (-not $Version) {
  $verLine = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "([^"]+)"'
  $cargo = $verLine.Matches[0].Groups[1].Value
  $sha = (git -C $root rev-parse --short HEAD 2>$null)
  $Version = if ($sha) { "$cargo-dev.$sha" } else { "$cargo-dev" }
}
if ($Version -match '^(\d+\.\d+\.\d+)') {
  $numeric = $Matches[1]
} else {
  throw "Version '$Version' does not start with x.y.z (needed for Inno AppVersion / VersionInfoVersion)"
}

$stage = Join-Path ([IO.Path]::GetTempPath()) "vivi-installer-stage-$Arch"
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory $stage | Out-Null
Copy-Item $Exe, $Mpv, "$PSScriptRoot\vivi.ico" $stage
Copy-Item "$root\README.md", "$root\LICENSE", "$root\CHANGELOG.md" $stage

# App-local VC++ runtime: only the DLLs vivi.exe actually imports.
$vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -property installationPath
$dumpbin = (Get-ChildItem "$vs\VC\Tools\MSVC\*\bin\Host*\*\dumpbin.exe" | Select-Object -First 1).FullName
$deps = & $dumpbin /dependents $Exe | ForEach-Object { $_.Trim() } | Where-Object { $_ -match '^(vcruntime|msvcp|concrt)\S*\.dll$' }
$crt = Get-ChildItem "$vs\VC\Redist\MSVC\*\$Arch\Microsoft.VC*.CRT" -Directory | Sort-Object FullName | Select-Object -Last 1
if (-not $crt) { throw "VC++ redist folder for $Arch not found under $vs\VC\Redist\MSVC" }
foreach ($d in $deps) {
  $src = Join-Path $crt.FullName $d
  if (-not (Test-Path $src)) { throw "$d (imported by vivi.exe) not in $($crt.FullName)" }
  Copy-Item $src $stage
}
Write-Host "VC++ runtime copied app-local from $($crt.FullName): $($deps -join ', ')"
Get-ChildItem $stage | Format-Table Name, Length

$iscc = (Get-Command iscc.exe -ErrorAction SilentlyContinue).Source
if (-not $iscc) { $iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" }
if (-not (Test-Path $iscc)) {
  choco install innosetup -y --no-progress | Out-Null
  $iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
}
& $iscc /Qp "/DAppVersion=$numeric" "/DNameVersion=$Version" "/DArch=$Arch" "/DStageDir=$stage" "/DOutputDir=$OutDir" "$PSScriptRoot\vivi.iss"
if ($LASTEXITCODE -ne 0) { throw "ISCC failed ($LASTEXITCODE)" }
Get-ChildItem "$OutDir\vivi-$Version-windows-$Arch-setup.exe"
