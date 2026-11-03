param([Parameter(Mandatory)][string]$Report, [Parameter(Mandatory)][string]$Tag, [string]$Dist='dist')
$ErrorActionPreference='Stop'
. "$PSScriptRoot/Common.ps1"
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$version=Get-AppVersion $Tag
if ($Tag -cne "v$version") { throw 'Tag must be vMAJOR.MINOR.PATCH' }
$record=Get-Content -LiteralPath (Join-Path $root $Report) -Raw | ConvertFrom-Json -AsHashtable
$source=Get-VerifiedArtifact $root $record $version
$distPath=[IO.Path]::GetFullPath((Join-Path $root $Dist))
if (Test-Path $distPath) { throw "Release output already exists: $distPath" }
New-Item -ItemType Directory -Path $distPath | Out-Null
$name="WirelessDeviceBatteryMonitor-v$version.exe"
$destination=Join-Path $distPath $name
Copy-Item -LiteralPath $source -Destination $destination
$hash=(Get-FileHash $destination -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::WriteAllText("$destination.sha256", "$hash  $name`n")
# Actual revisions are informational only: any current core branch tip is accepted.
Write-Json (Join-Path $distPath 'build-info.json') @{version=$version;tag=$Tag;app_commit=$record.source.app_commit;core_commit=$record.source.core_commit;lock_sha256=$record.source.lock_sha256;artifact=@{name=$name;sha256=$hash;bytes=(Get-Item $destination).Length};tests=@{app=$record['unit-app'].passed;core=$record['unit-core'].passed}}
Write-Host "Prepared $destination"
