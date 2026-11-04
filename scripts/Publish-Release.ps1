param([Parameter(Mandatory)][string]$Tag, [Parameter(Mandatory)][string]$Repository, [string]$Dist='dist')
$ErrorActionPreference='Stop'
. "$PSScriptRoot/Common.ps1"
$version=Get-AppVersion $Tag
if ($Tag -cne "v$version") { throw 'Tag must be vMAJOR.MINOR.PATCH' }
function Api([string[]]$Arguments) {
    $result=& gh api @Arguments
    if ($LASTEXITCODE -ne 0) { throw "GitHub API failed: $($Arguments[0])" }
    if ($result) { ($result -join "`n") | ConvertFrom-Json }
}
function JsonApi([string]$Method,[string]$Endpoint,$Payload) {
    $temp=[IO.Path]::GetTempFileName()
    try {
        [IO.File]::WriteAllText($temp,($Payload | ConvertTo-Json -Depth 20 -Compress))
        Api @($Endpoint,'--method',$Method,'--input',$temp)
    } finally { Remove-Item -LiteralPath $temp -Force }
}
$name="WirelessDeviceBatteryMonitor-v$version.exe"
$exe=Join-Path $Dist $name
$record=Get-Content (Join-Path $Dist 'build-info.json') -Raw | ConvertFrom-Json
$signed=Get-Content (Join-Path $Dist 'gpui-windows.json') -Raw | ConvertFrom-Json
$manifest=$signed.manifest | ConvertFrom-Json
$hash=(Get-FileHash $exe -Algorithm SHA256).Hash.ToLowerInvariant()
$size=(Get-Item $exe).Length
if ($record.tag -cne $Tag -or $record.artifact.name -cne $name -or $record.artifact.sha256 -cne $hash -or $record.artifact.bytes -ne $size) { throw 'Artifact identity mismatch' }
if ($manifest.version -cne $version -or $manifest.sha256 -cne $hash -or $manifest.size -ne $size -or $signed.signature -cnotmatch '^[0-9a-f]{128}$') { throw 'Signed manifest does not describe this executable' }
$commit=Api @("repos/$Repository/commits/$Tag")
if ($commit.sha -cne $record.app_commit) { throw 'Tag moved since this executable was built; refusing to publish stale bytes' }
$pages=Api @("repos/$Repository/releases?per_page=100",'--paginate','--slurp')
$matching=@(foreach($page in $pages) { foreach($release in $page) { if ($release.tag_name -ceq $Tag) { $release } } })
$published=@($matching | Where-Object { !$_.draft })
if ($published.Count -gt 1) { throw 'Multiple published releases share this tag' }
$release=$published | Select-Object -First 1
if (!$release) { $release=$matching | Select-Object -First 1 }
if (!$release) {
    $notes=JsonApi 'POST' "repos/$Repository/releases/generate-notes" @{tag_name=$Tag;target_commitish=$commit.sha}
    $release=JsonApi 'POST' "repos/$Repository/releases" @{tag_name=$Tag;target_commitish=$commit.sha;name=$Tag;body=$notes.body;draft=$true}
}
$endpoint="repos/$Repository/releases/$($release.id)"
# Upload first. A failed upload must not remove the working public executable.
# The temporary asset and previous asset are removed only after publication.
$suffix=[guid]::NewGuid().ToString('N')
$stagedName="$name.upload-$suffix"
$upload=Api @("https://uploads.github.com/repos/$Repository/releases/$($release.id)/assets?name=$stagedName",'--method','POST','--header','Content-Type: application/octet-stream','--input',$exe)
if ($upload.name -cne $stagedName -or $upload.state -cne 'uploaded' -or $upload.size -ne $size -or $upload.digest -cne "sha256:$hash") { throw 'Uploaded executable hash or size mismatch; previous download preserved' }
$marker='(?s)\s*<!-- gpui-update-manifest\s*\r?\n.*?\r?\n-->\s*'
$body=[regex]::Replace([string]$release.body,$marker,'').TrimEnd()
$envelope=$signed | ConvertTo-Json -Compress
$body+="`n`n<!-- gpui-update-manifest`n$envelope`n-->`n"
$previous=@($release.assets | Where-Object { $_.name -ceq $name })
$renamed=@()
$promoted=$false
try {
    foreach($asset in $previous) {
        JsonApi 'PATCH' "repos/$Repository/releases/assets/$($asset.id)" @{name="$name.previous-$suffix"} | Out-Null
        $renamed+=$asset
    }
    JsonApi 'PATCH' "repos/$Repository/releases/assets/$($upload.id)" @{name=$name} | Out-Null
    $promoted=$true
    JsonApi 'PATCH' $endpoint @{target_commitish=$commit.sha;body=$body;draft=$false;make_latest='true'} | Out-Null
} catch {
    $failure=$_
    # Rename rollback is best effort; GitHub does not offer an atomic asset/body
    # transaction. Keep all bytes if the server is unavailable during rollback.
    try {
        if ($promoted) { JsonApi 'PATCH' "repos/$Repository/releases/assets/$($upload.id)" @{name=$stagedName} | Out-Null }
        foreach($asset in $renamed) { JsonApi 'PATCH' "repos/$Repository/releases/assets/$($asset.id)" @{name=$name} | Out-Null }
        JsonApi 'PATCH' $endpoint @{target_commitish=$release.target_commitish;body=[string]$release.body;draft=[bool]$release.draft} | Out-Null
    } catch { Write-Warning "Publication rollback needs a retry; retained assets were not deleted: $_" }
    throw $failure
}
# Metadata stays in the CI artifact and signed release body; downloads contain only EXE.
$current=Api @($endpoint)
foreach($asset in $current.assets) {
    if ($asset.id -ne $upload.id) { Api @("repos/$Repository/releases/assets/$($asset.id)",'--method','DELETE') | Out-Null }
}
foreach($draft in $matching) {
    if ($draft.draft -and $draft.id -ne $release.id) { Api @("repos/$Repository/releases/$($draft.id)",'--method','DELETE') | Out-Null }
}
$current=Api @($endpoint)
if (@($current.assets).Count -ne 1 -or $current.assets[0].id -ne $upload.id -or $current.assets[0].digest -cne "sha256:$hash" -or $current.target_commitish -cne $commit.sha -or $current.body -cne $body) { throw 'Published release differs from the verified executable or metadata' }
Write-Host "Published $Tag at $($commit.sha): $name sha256:$hash (release $($release.id))"
