# Records USB traffic on every root hub with USBPcap, to find out what a
# vendor app sends to a device. Run from an elevated PowerShell:
#
#   powershell -ExecutionPolicy Bypass -File tools\usb-capture.ps1 -Seconds 40
#
# Needs USBPcap (https://github.com/desowin/usbpcap/releases). After
# installing it, replug the device or reboot so the filter driver attaches.
# Output: one .pcap per root hub in the folder given by -OutDir.

param(
    [int]$Seconds = 40,
    [string]$OutDir = "$env:TEMP\trayjuice-usb"
)

$cmd = "$env:ProgramFiles\USBPcap\USBPcapCMD.exe"
if (-not (Test-Path $cmd)) {
    Write-Error "USBPcapCMD.exe not found at $cmd. Install USBPcap first."
    exit 1
}
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) {
    Write-Error "Run this script from an elevated (administrator) PowerShell."
    exit 1
}

New-Item -ItemType Directory -Force $OutDir | Out-Null
Remove-Item "$OutDir\*.pcap" -ErrorAction SilentlyContinue

# USBPcap exposes one control device per root hub: \\.\USBPcap1, 2, ...
$captures = @()
foreach ($n in 1..16) {
    $device = "\\.\USBPcap$n"
    $out = Join-Path $OutDir "hub$n.pcap"
    $p = Start-Process $cmd -ArgumentList "-d", $device, "-o", "`"$out`"", "-A" `
        -PassThru -WindowStyle Hidden
    Start-Sleep -Milliseconds 300
    if ($p.HasExited) { continue }
    $captures += $p
    Write-Host "capturing $device -> $out"
}
if ($captures.Count -eq 0) {
    Write-Error "No root hub could be opened. Replug the device or reboot after installing USBPcap."
    exit 1
}

Write-Host ""
Write-Host "Recording for $Seconds seconds. Do the thing you want to capture NOW."
Start-Sleep -Seconds $Seconds

$captures | ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
Start-Sleep -Milliseconds 500
Write-Host ""
Get-ChildItem "$OutDir\*.pcap" | ForEach-Object { Write-Host ("{0}  {1} bytes" -f $_.FullName, $_.Length) }
Write-Host "Done."
