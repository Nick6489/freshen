param([switch]$ReinstallVersion1)
$ErrorActionPreference = 'Stop'
$repo = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$installed = Join-Path $env:LOCALAPPDATA 'Programs\FreshenNativeTest'
$state = Join-Path $env:LOCALAPPDATA 'FreshenNativeDialogTestServicing'
$roots = @($installed, $state)
$running = @(Get-Process | Where-Object {
    try {
        $imagePath = $_.Path
        $imagePath -and @($roots | Where-Object { $imagePath.StartsWith($_ + '\', [StringComparison]::OrdinalIgnoreCase) }).Count -gt 0
    } catch { $false }
})
if ($running.Count -gt 0) { throw 'Close the test app and completed update dialog, then wait for its helper to exit. This script never kills processes.' }
$transaction = Join-Path $state '.freshen\transaction'
if (Test-Path -LiteralPath $transaction) {
    $archive = Join-Path $repo ('target\native-ui-demo\log-archive\' + (Get-Date -Format 'yyyyMMdd-HHmmss-fff'))
    New-Item -ItemType Directory -Force $archive | Out-Null
    Copy-Item -LiteralPath $transaction -Destination $archive -Recurse
    & (Join-Path $repo 'target\release\freshen-release.exe') discard-finished $state
    if ($LASTEXITCODE -ne 0) { throw 'Transaction retained: active or unknown Inno outcomes require inspection; automatic rollback is unavailable.' }
    Write-Output "Preserved transaction evidence in $archive"
}
if ($ReinstallVersion1) {
    $installer = Join-Path $repo 'target\native-ui-demo\installers\FreshenNativeTest-1.0.0-x64.exe'
    if (!(Test-Path -LiteralPath $installer)) { throw 'Run build-demo.ps1 first' }
    $process = Start-Process -FilePath $installer -ArgumentList @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/CURRENTUSER') -WindowStyle Hidden -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "Reinstalling the disposable version 1 fixture failed: $($process.ExitCode)" }
    Write-Output 'The disposable app is back at version 1. Its existing AppId and directory were retained.'
}
