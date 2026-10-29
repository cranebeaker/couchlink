# Builds couchlink_hook.dll and test_target.exe for x64 and x86 into hook/build/<arch>/.
# Needs Visual Studio 2022 (Build Tools are enough) with the C++ workload, and CMake.
# Usage: powershell -File hook/build.ps1 [-Arch x64|x86|all] [-Config Release|Debug] [-Clean] [-OutDir <folder>]
# -OutDir (default hook/build) is where x64/ and x86/ are written; use another folder while a game that has the
# DLL loaded is still running (a loaded DLL cannot be overwritten).
param(
    [ValidateSet('x64', 'x86', 'all')][string]$Arch = 'all',
    [ValidateSet('Release', 'Debug')][string]$Config = 'Release',
    [switch]$Clean,
    [string]$OutDir = ''
)
$ErrorActionPreference = 'Stop'
# A machine-wide Android toolchain file would hijack the configure step.
Remove-Item Env:CMAKE_TOOLCHAIN_FILE -ErrorAction SilentlyContinue
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$cmake = (Get-Command cmake -ErrorAction SilentlyContinue)
if (-not $cmake) { throw 'cmake not found on PATH' }
if (-not $OutDir) { $OutDir = Join-Path $here 'build' }
$OutDir = [System.IO.Path]::GetFullPath($OutDir)
$archs = if ($Arch -eq 'all') { @('x64', 'x86') } else { @($Arch) }
foreach ($a in $archs) {
    $platform = if ($a -eq 'x64') { 'x64' } else { 'Win32' }
    $obj = Join-Path $OutDir "obj-$a"
    if ($Clean -and (Test-Path $obj)) { Remove-Item -Recurse -Force $obj }
    & cmake -S $here -B $obj -G 'Visual Studio 17 2022' -A $platform "-DCL_ARCH=$a" "-DCL_OUT_ROOT=$OutDir"
    if ($LASTEXITCODE -ne 0) { throw "cmake configure failed for $a" }
    & cmake --build $obj --config $Config --parallel
    if ($LASTEXITCODE -ne 0) { throw "build failed for $a" }
    $dll = Join-Path $OutDir "$a/couchlink_hook.dll"
    if (-not (Test-Path $dll)) { throw "missing output $dll" }
    Write-Host "built $dll"
}
