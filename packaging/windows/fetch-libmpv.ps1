# Downloads libmpv (shinchiro/mpv-winbuild-cmake "mpv-dev" build) into .\mpv and creates the
# MSVC import library mpv.lib for it, then adds the folder to RUSTFLAGS (and PATH, for tests).
#   pwsh packaging/windows/fetch-libmpv.ps1 -Arch x64     # or arm64
# Needs GH_TOKEN for `gh release download`.
param([ValidateSet('x64', 'arm64')] [string] $Arch = 'x64', [switch] $AddToPath)
$ErrorActionPreference = 'Stop'
$pattern = @{ x64 = 'mpv-dev-x86_64-2*'; arm64 = 'mpv-dev-aarch64-2*' }[$Arch]
gh release download -R shinchiro/mpv-winbuild-cmake -p $pattern -O mpv-dev.7z --clobber
7z x mpv-dev.7z -ompv -y | Out-Null
Get-ChildItem mpv
$vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -property installationPath
# dumpbin and lib from any host folder (an x64 host runs on arm64 through emulation).
$tool = { param($n) (Get-ChildItem "$vs\VC\Tools\MSVC\*\bin\Host*\*\$n" | Sort-Object { $_.FullName -notmatch "Host$Arch" } | Select-Object -First 1).FullName }
$dumpbin = & $tool 'dumpbin.exe'
$lib = & $tool 'lib.exe'
$dll = (Get-ChildItem mpv\libmpv-2.dll).FullName
if (-not (Test-Path mpv\mpv.def)) {
  $names = & $dumpbin /exports $dll | ForEach-Object { if ($_ -match '^\s+\d+\s+[0-9A-F]+\s+[0-9A-F]{8}\s+(\S+)') { $Matches[1] } }
  (@('EXPORTS') + $names) | Set-Content mpv\mpv.def
}
& $lib /def:mpv\mpv.def /name:libmpv-2.dll "/machine:$Arch" /out:mpv\mpv.lib
if ($LASTEXITCODE -ne 0) { throw "lib.exe failed ($LASTEXITCODE)" }
$dir = (Resolve-Path mpv).Path
if ($env:GITHUB_ENV) {
  "RUSTFLAGS=-L native=$dir" | Out-File -Append $env:GITHUB_ENV
  if ($AddToPath) { $dir | Out-File -Append $env:GITHUB_PATH }
}
