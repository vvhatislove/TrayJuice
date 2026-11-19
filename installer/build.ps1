# Builds dist\TrayJuice-<version>-setup.exe with Inno Setup.
#
#   installer\build.ps1                      # target\release\trayjuice.exe
#   installer\build.ps1 -Exe signed\trayjuice.exe
#
# The version comes from Cargo.toml. Inno Setup 6 has to be installed
# (https://jrsoftware.org/isinfo.php); on a GitHub runner it is added with
# Chocolatey when missing.
param(
    [string]$Exe = "$PSScriptRoot\..\target\release\trayjuice.exe"
)

$ErrorActionPreference = 'Stop'
$root = Resolve-Path "$PSScriptRoot\.."
$exePath = (Resolve-Path $Exe).Path

$manifest = cargo metadata --no-deps --format-version 1 --manifest-path "$root\Cargo.toml" | ConvertFrom-Json
$version = $manifest.packages[0].version

function Find-Iscc {
    $onPath = Get-Command iscc -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    foreach ($dir in @(${env:ProgramFiles(x86)}, $env:ProgramFiles, "$env:LOCALAPPDATA\Programs")) {
        $candidate = Join-Path $dir 'Inno Setup 6\ISCC.exe'
        if (Test-Path $candidate) { return $candidate }
    }
    return $null
}

$iscc = Find-Iscc
if (-not $iscc -and $env:GITHUB_ACTIONS) {
    choco install innosetup --yes --no-progress
    if ($LASTEXITCODE -ne 0) { throw "could not install Inno Setup" }
    $iscc = Find-Iscc
}
if (-not $iscc) {
    throw "Inno Setup 6 (ISCC.exe) was not found"
}

& $iscc "/DAppVersion=$version" "/DAppExe=$exePath" "$root\installer\trayjuice.iss"
if ($LASTEXITCODE -ne 0) { throw "ISCC failed with exit code $LASTEXITCODE" }

$setup = Join-Path $root "dist\TrayJuice-$version-setup.exe"
if (-not (Test-Path $setup)) { throw "$setup was not written" }
Write-Output $setup
