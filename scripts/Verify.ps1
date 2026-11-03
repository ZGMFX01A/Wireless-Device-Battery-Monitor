param([string]$OutDir, [switch]$Ui)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/Common.ps1"
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$core = [IO.Path]::GetFullPath((Join-Path $root '../mouse-battery-core'))
$version = Get-AppVersion $env:APP_VERSION
if (!$OutDir) { $OutDir = 'out/review-' + (Get-Date -Format 'yyyyMMdd-HHmmss-fff') }
$out = [IO.Path]::GetFullPath((Join-Path $root $OutDir))
if (!$out.StartsWith((Join-Path $root 'out') + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Output must be under out/' }
if (Test-Path $out) { throw "Output already exists: $out" }
$evidence = Join-Path $out 'evidence'
New-Item -ItemType Directory -Path $evidence | Out-Null
$record = [ordered]@{ok=$false;version=$version;physical_devices_tested=$false;ui_requested=$Ui.IsPresent;started=[DateTime]::UtcNow.ToString('o')}
$oldVersion = $env:APP_VERSION
$env:APP_VERSION = $version
function Step([string]$Name, [string]$File, [string[]]$Arguments, [int]$Seconds = 900) {
    $record[$Name] = @{command=@($File)+$Arguments;exit_code=$null}
    $text = Invoke-Tool -File $File -Arguments $Arguments -WorkingDirectory $root -Log (Join-Path $evidence "$Name.log") -TimeoutSeconds $Seconds
    $record[$Name].exit_code = 0
    Write-Host "$Name : OK"
    return $text
}
try {
    if (!(Test-Path "$core/Cargo.toml")) { throw 'Missing sibling mouse-battery-core checkout' }
    # Resolve normally first: latest core may legitimately change its dependencies.
    $metadata = (Step 'metadata' 'cargo' @('metadata','--no-deps','--format-version','1')) | ConvertFrom-Json
    $record.source = @{app_commit=(Invoke-Tool git @('-C',$root,'rev-parse','HEAD')).Trim();core_commit=(Invoke-Tool git @('-C',$core,'rev-parse','HEAD')).Trim()}
    Step 'format-app' 'cargo' @('fmt','--package','mouse-battery-gpui','--','--check') | Out-Null
    Step 'format-core' 'cargo' @('fmt','--manifest-path',"$core/Cargo.toml",'--package','battery-protocol','--','--check') | Out-Null
    Step 'script-tests' 'pwsh' @('-NoProfile','-NonInteractive','-File',"$PSScriptRoot/Test-Tools.ps1") | Out-Null
    Step 'clippy' 'cargo' @('clippy','--all-targets','-p','mouse-battery-gpui','-p','battery-protocol','--','-D','warnings') | Out-Null
    $total = 0
    foreach ($pair in @(@('unit-core','battery-protocol'),@('unit-app','mouse-battery-gpui'))) {
        $text = Step $pair[0] 'cargo' @('test','--locked','--all-targets','-p',$pair[1])
        $count = 0
        foreach($m in [regex]::Matches($text,'test result: ok\. (\d+) passed')) { $count += [int]$m.Groups[1].Value }
        if (!$count) { throw "No tests ran: $($pair[0])" }
        $record[$pair[0]].passed=$count; $total += $count
    }
    $record.unit = @{passed=$total}
    Step 'build' 'cargo' @('build','--release','--locked','--bin','mouse-battery-gpui') 1800 | Out-Null
    $exe = Join-Path $metadata.target_directory 'release/mouse-battery-gpui.exe'
    if ((Step 'version-check' $exe @('--version') 30).Trim() -cne $version) { throw 'Executable version mismatch' }
    Step 'smoke' $exe @('--smoke-test') 30 | Out-Null
    $tree = Step 'deps' 'cargo' @('tree','--locked','-p','mouse-battery-gpui','-e','normal')
    if ($tree -match '\b(?:tauri|wry|webview2-com|pyo3|pythonnet) v\d') { throw 'Unexpected runtime dependency' }
    if ($Ui) {
        Step 'build-ui-probe' 'cargo' @('build','--locked','--example','ui_probe') 1200 | Out-Null
        $probe = Join-Path $metadata.target_directory 'debug/examples/ui_probe.exe'
        $json = Step 'ui' $probe @($exe,(Join-Path $evidence 'ui')) 240
        $record.ui.results = $json.Trim() | ConvertFrom-Json -AsHashtable
        if (!$record.ui.results.ok) { throw 'UI regression failed' }
    }
    $destination=Join-Path $out "WirelessDeviceBatteryMonitor-v$version.exe"
    Copy-Item -LiteralPath $exe -Destination $destination
    $record.artifact=@{path=[IO.Path]::GetRelativePath($root,$destination).Replace('\','/');bytes=(Get-Item $destination).Length;sha256=(Get-FileHash $destination -Algorithm SHA256).Hash.ToLowerInvariant()}
    $record.source.lock_sha256=(Get-FileHash (Join-Path $metadata.workspace_root 'Cargo.lock') -Algorithm SHA256).Hash.ToLowerInvariant()
    $record.ok=$true
} finally {
    $env:APP_VERSION=$oldVersion
    $record.finished=[DateTime]::UtcNow.ToString('o')
    Write-Json (Join-Path $evidence 'verification.json') $record
    Write-Host "EVIDENCE $evidence"
}
