# Fetch the AI engine for Windows: Microsoft's DirectML build of ONNX
# Runtime (it also runs on the processor) and the DirectML library it
# needs, from nuget.org, checked against pinned checksums.
#
#   scripts\get-onnxruntime.ps1 -Dir <folder>
#
# Puts onnxruntime.dll, onnxruntime_providers_shared.dll and DirectML.dll
# into <folder> (next to voice-changer.exe is where the program looks), and
# their licences into <folder>\licenses.
param(
    [Parameter(Mandatory = $true)][string]$Dir
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$packages = @(
    @{
        Name   = "microsoft.ml.onnxruntime.directml"
        Version = "1.22.0"
        Sha256 = "29f9872d786236b79aa83f94482f3a17c14297e4833768d6d0ed4883ee732e60"
        Files  = @(
            "runtimes/win-x64/native/onnxruntime.dll",
            "runtimes/win-x64/native/onnxruntime_providers_shared.dll"
        )
        Licenses = @{ "LICENSE" = "onnxruntime-LICENSE.txt"; "ThirdPartyNotices.txt" = "onnxruntime-ThirdPartyNotices.txt" }
    },
    @{
        Name   = "microsoft.ai.directml"
        Version = "1.15.4"
        Sha256 = "4e7cb7ddce8cf837a7a75dc029209b520ca0101470fcdf275c1f49736a3615b9"
        Files  = @("bin/x64-win/DirectML.dll")
        Licenses = @{ "LICENSE.txt" = "DirectML-LICENSE.txt" }
    }
)

New-Item -ItemType Directory -Force -Path $Dir | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $Dir "licenses") | Out-Null
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("vc-ort-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $work | Out-Null
try {
    foreach ($p in $packages) {
        $url = "https://api.nuget.org/v3-flatcontainer/$($p.Name)/$($p.Version)/$($p.Name).$($p.Version).nupkg"
        $zip = Join-Path $work "$($p.Name).zip"
        Write-Host "Downloading $($p.Name) $($p.Version)"
        Invoke-WebRequest -Uri $url -OutFile $zip
        $hash = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
        if ($hash -ne $p.Sha256) {
            throw "$($p.Name): checksum is $hash, expected $($p.Sha256). Not using this download."
        }
        $out = Join-Path $work $p.Name
        Expand-Archive -Path $zip -DestinationPath $out -Force
        foreach ($f in $p.Files) {
            Copy-Item (Join-Path $out $f) $Dir -Force
        }
        foreach ($l in $p.Licenses.GetEnumerator()) {
            Copy-Item (Join-Path $out $l.Key) (Join-Path $Dir "licenses\$($l.Value)") -Force
        }
    }
}
finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
Get-ChildItem $Dir -Filter *.dll | ForEach-Object { Write-Host ("  {0}  {1:N1} MB" -f $_.Name, ($_.Length / 1MB)) }
