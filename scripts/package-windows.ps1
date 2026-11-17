# Assemble the Windows download: a folder (and a zip of it) that runs from
# wherever it is unpacked. Nothing is installed and nothing on the system
# is changed.
#
#   cargo build --release -p vc-app -p vc-plugin
#   scripts\package-windows.ps1 [-Out dist]
param(
    [string]$Out = "dist",
    [string]$Target = "target\release"
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$name = "voice-changer-windows-x64"
$stage = Join-Path $Out $name
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Copy-Item (Join-Path $root "$Target\voice-changer.exe") $stage
# The CLAP plugin is the plugin library under another name.
Copy-Item (Join-Path $root "$Target\vc_plugin.dll") (Join-Path $stage "vc-plugin.clap")
& (Join-Path $PSScriptRoot "get-onnxruntime.ps1") -Dir $stage
Copy-Item (Join-Path $root "LICENSE") (Join-Path $stage "licenses\voice-changer-LICENSE.txt")
Copy-Item (Join-Path $root "packaging\windows\README.txt") $stage

$zip = Join-Path $Out "$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $stage -DestinationPath $zip
Write-Host "Packaged $zip"
Get-ChildItem $stage -Recurse -File | ForEach-Object { Write-Host ("  {0}" -f $_.FullName.Substring((Resolve-Path $stage).Path.Length + 1)) }
