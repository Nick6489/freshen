param([string]$SourceFeed = (Join-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) 'target\native-ui-demo\feed'))
$ErrorActionPreference = 'Stop'
$destination = Join-Path $env:LOCALAPPDATA 'FreshenNativeDialogTestFeed'
foreach ($name in @('update.zip','manifest.json','manifest.json.sig')) {
    if (!(Test-Path -LiteralPath (Join-Path $SourceFeed $name))) { throw "Missing feed file: $name" }
}
New-Item -ItemType Directory -Force $destination | Out-Null
# Replace payload first and manifest last. Activation is done while no check is in flight.
foreach ($name in @('update.zip','manifest.json.sig','manifest.json')) {
    Copy-Item -LiteralPath (Join-Path $SourceFeed $name) -Destination (Join-Path $destination $name)
}
Write-Output "Version 2 is available in $destination. Choose Check for updates in Freshen Native Test."
