# Offline GitHub API simulation. No network requests, credentials or real releases.
$ErrorActionPreference='Stop'
$root=Join-Path ([IO.Path]::GetTempPath()) ('battery-publish-test-'+[guid]::NewGuid().ToString('N'))
$oldMock=$global:WdbmPublishTest
function gh {
    param([Parameter(ValueFromRemainingArguments)][string[]]$Arguments)
    $global:LASTEXITCODE=0
    $s=$global:WdbmPublishTest
    if($Arguments[0] -cne 'api'){throw 'Unexpected CLI operation'}
    $endpoint=$Arguments[1]; $method='GET'; $payload=$null
    $index=[Array]::IndexOf($Arguments,'--method'); if($index -ge 0){$method=$Arguments[$index+1]}
    $index=[Array]::IndexOf($Arguments,'--input'); if($index -ge 0 -and $endpoint -notlike 'https://uploads.*'){$payload=Get-Content $Arguments[$index+1] -Raw | ConvertFrom-Json -AsHashtable}
    $s.calls.Add("$method $endpoint")
    if($endpoint -like '*/commits/*'){return (@{sha=$s.sha}|ConvertTo-Json -Compress)}
    if($endpoint -like '*/releases?per_page=*'){return (ConvertTo-Json -InputObject @(@($s.release)) -Depth 15 -Compress)}
    if($endpoint -like 'https://uploads.*'){
        if($s.mode -eq 'upload-failure'){$global:LASTEXITCODE=1;return}
        $name=[Uri]::UnescapeDataString(($endpoint -split 'name=',2)[1])
        $asset=@{id=22;name=$name;state='uploaded';size=$s.size;digest="sha256:$($s.hash)"}
        if($s.mode -eq 'bad-hash'){$asset.digest='sha256:invalid'}
        $s.release.assets.Add($asset)
        return ($asset|ConvertTo-Json -Compress)
    }
    if($endpoint -match '/releases/assets/(\d+)$'){
        $id=[int]$Matches[1];$asset=$s.release.assets|Where-Object id -EQ $id
        if(!$asset){throw 'Unknown fixture asset'}
        if($method -eq 'DELETE'){[void]$s.release.assets.Remove($asset);return}
        if($method -ne 'PATCH'){throw 'Unexpected asset method'}
        $asset.name=$payload.name; return ($asset|ConvertTo-Json -Compress)
    }
    if($endpoint -match '/releases/11$'){
        if($method -eq 'PATCH'){
            if($s.mode -eq 'body-failure' -and !$s.failedBody -and $payload.body -like '*gpui-update-manifest*'){
                $s.failedBody=$true; $global:LASTEXITCODE=1; return
            }
            foreach($key in $payload.Keys){$s.release[$key]=$payload[$key]}
        }
        return ($s.release|ConvertTo-Json -Depth 15 -Compress)
    }
    throw "Unexpected offline API operation: $method $endpoint"
}
try {
    New-Item -ItemType Directory -Path $root|Out-Null
    $name='WirelessDeviceBatteryMonitor-v3.0.0.exe';$exe=Join-Path $root $name
    [IO.File]::WriteAllText($exe,'not an executable; offline publish fixture')
    $hash=(Get-FileHash $exe -Algorithm SHA256).Hash.ToLowerInvariant();$size=(Get-Item $exe).Length
    $sha='a'*40
    @{tag='v3.0.0';app_commit=$sha;artifact=@{name=$name;sha256=$hash;bytes=$size}}|ConvertTo-Json -Depth 10|Set-Content (Join-Path $root 'build-info.json') -Encoding utf8
    @{manifest=(@{version='3.0.0';sha256=$hash;size=$size}|ConvertTo-Json -Compress);signature=('0'*128)}|ConvertTo-Json|Set-Content (Join-Path $root 'gpui-windows.json') -Encoding utf8
    foreach($mode in 'upload-failure','bad-hash','body-failure','success'){
        $assets=[Collections.Generic.List[object]]::new();$assets.Add(@{id=1;name=$name;digest='sha256:old';size=10;state='uploaded'})
        $global:WdbmPublishTest=@{mode=$mode;sha=$sha;hash=$hash;size=$size;calls=[Collections.Generic.List[string]]::new();failedBody=$false;release=@{id=11;tag_name='v3.0.0';target_commitish=$sha;body='Original release notes';draft=$false;assets=$assets}}
        $failed=$false
        try { & "$PSScriptRoot/Publish-Release.ps1" -Tag v3.0.0 -Repository fixture/test -Dist $root | Out-Null }
        catch { $failed=$true; if($mode -eq 'success'){throw} }
        $s=$global:WdbmPublishTest
        if($mode -eq 'success'){
            if($failed -or $s.release.assets.Count -ne 1 -or $s.release.assets[0].id -ne 22 -or $s.release.assets[0].name -cne $name){throw 'Successful release must contain one verified executable'}
        } else {
            if(!$failed){throw "Expected $mode to fail"}
            $old=$s.release.assets|Where-Object id -EQ 1
            if(!$old -or $old.name -cne $name -or $s.release.body -cne 'Original release notes'){throw "Previous release damaged by $mode"}
            if(@($s.calls|Where-Object{$_ -like 'DELETE *'}).Count){throw "Failed publication deleted an existing asset: $mode"}
        }
    }
    Write-Host '4 offline release-publication scenarios passed'
} finally {
    $global:WdbmPublishTest=$oldMock
    if(Test-Path $root){Remove-Item $root -Recurse -Force}
}
