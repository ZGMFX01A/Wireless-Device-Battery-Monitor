param([Parameter(Mandatory)][string]$Report)
$ErrorActionPreference='Stop'
. "$PSScriptRoot/Common.ps1"
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$reportPath=[IO.Path]::GetFullPath((Join-Path $root $Report))
$record=Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json -AsHashtable
$exe=Get-VerifiedArtifact $root $record $record.version
$out=Split-Path $exe
$package=Join-Path $out 'package'
New-Item -ItemType Directory -Path $package -ErrorAction Stop | Out-Null
Copy-Item $exe,"$root/LICENSE","$root/README.md" $package
Copy-Item "$root/assets" (Join-Path $package 'assets') -Recurse
Copy-Item "$root/third-party" (Join-Path $package 'third-party') -Recurse
Copy-Item (Split-Path $reportPath) (Join-Path $package 'evidence') -Recurse
$zip=Join-Path $out "WirelessDeviceBatteryMonitor-v$($record.version)-review.zip"
Compress-Archive -Path "$package/*" -DestinationPath $zip
Write-Host "PACKAGE $zip"
