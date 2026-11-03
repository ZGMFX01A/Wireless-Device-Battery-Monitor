$ErrorActionPreference='Stop'
. "$PSScriptRoot/Common.ps1"
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$count=0
function Check([bool]$Condition,[string]$Message) { if (!$Condition) { throw $Message }; $script:count++ }
function Reject([scriptblock]$Action) { $failed=$false; try { & $Action | Out-Null } catch { $failed=$true }; Check $failed 'Expected invalid input to fail' }

foreach($good in '0.1.3','v2.8.1','65535.0.0') { Check ((Get-AppVersion $good) -ceq ($good -creplace '^v','')) 'Version parsing failed' }
foreach($bad in '1.0','01.2.3','1.2.3; exit 0','1.2.65536','1.0.0-preview') { Reject { Get-AppVersion $bad } }
foreach($script in Get-ChildItem $PSScriptRoot -Filter '*.ps1') {
    $tokens=$null; $errors=$null
    [Management.Automation.Language.Parser]::ParseFile($script.FullName,[ref]$tokens,[ref]$errors) | Out-Null
    Check ($errors.Count -eq 0) "PowerShell parse error: $($script.Name): $errors"
}
foreach($name in 'ci.yml','release.yml') {
    $text=[IO.File]::ReadAllText((Join-Path $root ".github/workflows/$name"))
    Check ($text -notmatch 'setup-python|\bpython(?:3)?\s|private-core-ref|check-core-pin|steps\.core\.outputs\.sha') "Old toolchain or core pin remains in $name"
    $checkout=[regex]::Match($text,'(?ms)- name: Checkout latest private core\r?\n(.*?)(?=\r?\n\s+- name:|\z)').Groups[1].Value
    Check ($checkout -match 'repository:' -and $checkout -notmatch '(?m)^\s+ref:') "Core checkout must use its latest default branch: $name"
}
foreach($folder in 'scripts','tests') {
    if (Test-Path (Join-Path $root $folder)) {
        Check (@(Get-ChildItem (Join-Path $root $folder) -Filter '*.py' -File -Recurse).Count -eq 0) "Active Python file remains in $folder"
    }
}
Reject { Invoke-Tool 'pwsh' @('-NoProfile','-NonInteractive','-Command','exit 23') }
$temp=Join-Path ([IO.Path]::GetTempPath()) ('battery-tools-'+[guid]::NewGuid().ToString('N'))
try {
    New-Item -ItemType Directory -Path "$temp/out" | Out-Null
    $exe="$temp/out/WirelessDeviceBatteryMonitor-v1.2.3.exe"
    [IO.File]::WriteAllText($exe,'not an executable; artifact validation fixture')
    $record=[ordered]@{ok=$true;version='1.2.3';source=@{core_commit='any-current-default-branch-tip'};artifact=@{path='out/WirelessDeviceBatteryMonitor-v1.2.3.exe';bytes=(Get-Item $exe).Length;sha256=(Get-FileHash $exe).Hash}}
    foreach($stage in 'clippy','unit-core','unit-app','build','version-check','smoke') {$record[$stage]=@{exit_code=0}}
    Check ((Get-VerifiedArtifact $temp $record '1.2.3') -eq [IO.Path]::GetFullPath($exe)) 'Artifact validation failed'
    $record.source.core_commit='a-different-tip'; Check ((Get-VerifiedArtifact $temp $record '1.2.3') -eq [IO.Path]::GetFullPath($exe)) 'Core revision must not restrict packaging'
    $record.ok=$false; Reject { Get-VerifiedArtifact $temp $record '1.2.3' }; $record.ok=$true
    [IO.File]::AppendAllText($exe,'changed'); Reject { Get-VerifiedArtifact $temp $record '1.2.3' }
} finally { if(Test-Path $temp) { Remove-Item -LiteralPath $temp -Recurse -Force } }
Write-Host "$count PowerShell checks passed"
