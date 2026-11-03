# Shared build helpers. No source-revision allowlist or interpreter dependency.
$ErrorActionPreference = 'Stop'

function Get-AppVersion([string]$Value) {
    if (!$Value) {
        $manifest = [IO.File]::ReadAllText((Join-Path $PSScriptRoot '../Cargo.toml'))
        $Value = [regex]::Match($manifest, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
    }
    $version = $Value.Trim() -creplace '^[vV]', ''
    if ($version -cnotmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') {
        throw 'Version must be MAJOR.MINOR.PATCH'
    }
    foreach ($part in $version.Split('.')) {
        if ($part.Length -gt 5 -or [uint32]$part -gt 65535) { throw 'Windows version field overflow' }
    }
    return $version
}

function Invoke-Tool {
    param([string]$File, [string[]]$Arguments = @(), [string]$WorkingDirectory = (Get-Location).Path,
          [string]$Log, [int]$TimeoutSeconds = 900)
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $File
    $info.WorkingDirectory = $WorkingDirectory
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.StandardOutputEncoding = [Text.UTF8Encoding]::new($false)
    $info.StandardErrorEncoding = [Text.UTF8Encoding]::new($false)
    foreach ($arg in $Arguments) { $info.ArgumentList.Add($arg) }
    $process = [Diagnostics.Process]::Start($info)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (!$process.WaitForExit($TimeoutSeconds * 1000)) {
            $process.Kill($true)
            $process.WaitForExit()
            if ($Log) { [IO.File]::WriteAllText($Log, $stdout.GetAwaiter().GetResult() + $stderr.GetAwaiter().GetResult()) }
            throw "Timed out: $File $($Arguments -join ' ')"
        }
        $text = $stdout.GetAwaiter().GetResult()
        $errors = $stderr.GetAwaiter().GetResult()
        if ($Log) { [IO.File]::WriteAllText($Log, $text + $errors) }
        if ($process.ExitCode -ne 0) { throw "Command failed ($($process.ExitCode)): $File $($Arguments -join ' ')`n$text`n$errors" }
        return $text
    } finally { $process.Dispose() }
}

function Write-Json([string]$Path, $Value) {
    [IO.File]::WriteAllText($Path, ($Value | ConvertTo-Json -Depth 30))
}

function Get-VerifiedArtifact([string]$Root, $Record, [string]$Version) {
    if ($Record.ok -ne $true -or $Record.version -cne (Get-AppVersion $Version)) { throw 'Verification did not pass for this version' }
    foreach ($step in 'clippy','unit-core','unit-app','build','version-check','smoke') {
        if (!$Record.Contains($step) -or $Record[$step].exit_code -ne 0) { throw "Missing successful stage: $step" }
    }
    $file = [IO.Path]::GetFullPath((Join-Path $Root $Record.artifact.path))
    $out = [IO.Path]::GetFullPath((Join-Path $Root 'out')) + [IO.Path]::DirectorySeparatorChar
    if (!$file.StartsWith($out, [StringComparison]::OrdinalIgnoreCase)) { throw 'Artifact is outside out/' }
    if ((Get-Item -LiteralPath $file).Length -ne $Record.artifact.bytes -or
        (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ine $Record.artifact.sha256) { throw 'Artifact no longer matches the tested file' }
    return $file
}
