param([string]$Iscc = "$env:LOCALAPPDATA\Programs\Inno Setup 7\ISCC.exe")
$ErrorActionPreference = 'Stop'
$repo = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$output = Join-Path $repo 'target\native-ui-demo'
function Invoke-Cargo {
    & cargo @args
    if ($LASTEXITCODE -ne 0) { throw "Cargo failed: $args" }
}
if (!(Test-Path -LiteralPath $Iscc)) { throw 'Pass -Iscc with the Inno 7 ISCC.exe path' }
New-Item -ItemType Directory -Force "$output\private", "$output\v1", "$output\v2", "$output\installers", "$output\payload", "$output\feed" | Out-Null
Push-Location $repo
$priorVersion = $env:FRESHEN_UI_VERSION
$priorKey = $env:FRESHEN_UI_PUBLIC_KEY_FILE
try {
    Invoke-Cargo build --locked --release -p freshen --bin freshen-helper --bin freshen-release
    $publisherPrivate = "$output\private\publisher.key"
    $publisherPublic = "$output\private\publisher.pub"
    if ((Test-Path $publisherPrivate) -ne (Test-Path $publisherPublic)) { throw 'The demo publisher key pair is incomplete; restore both files or use a fresh output directory' }
    if (!(Test-Path $publisherPrivate)) {
        & .\target\release\freshen-release.exe keygen $publisherPrivate $publisherPublic
        if ($LASTEXITCODE -ne 0) { throw 'Publisher key generation failed' }
    }
    & .\inno\build-bridge.ps1
    $env:FRESHEN_UI_PUBLIC_KEY_FILE = $publisherPublic
    foreach ($entry in @(@('1.0.0','v1'), @('2.0.0','v2'))) {
        $env:FRESHEN_UI_VERSION = $entry[0]
        Invoke-Cargo build --locked --release -p freshen-native-test-app
        $staging = Join-Path $output $entry[1]
        Copy-Item -LiteralPath target\release\freshen-native-test.exe -Destination $staging
        Copy-Item -LiteralPath target\release\freshen-helper.exe -Destination $staging
        & $Iscc "/DAppVersion=$($entry[0])" "/DSourceDir=$staging" "/DOutputDir=$output\installers" "/DFreshenInnoDll=$repo\target\x86_64-pc-windows-msvc\release\FreshenInno.dll" native-test-app\inno\native-test.iss
        if ($LASTEXITCODE -ne 0) { throw "Installer compilation failed: $($entry[0])" }
    }
    Copy-Item -LiteralPath "$output\installers\FreshenNativeTest-2.0.0-x64.exe" -Destination "$output\payload\setup.exe"
    # Only replace these named generated files, never recursively remove output directories.
    foreach ($name in @('update.zip','manifest.json','manifest.json.sig')) {
        $generated = Join-Path "$output\feed" $name
        if (Test-Path -LiteralPath $generated) { Remove-Item -LiteralPath $generated }
    }
    & .\target\release\freshen-release.exe pack "$output\payload" "$output\feed\update.zip" https://freshen-ui-test.invalid/update.zip x86_64-pc-windows-msvc inno freshen-native-dialog-test 2.0.0 stable "$output\feed\manifest.json"
    if ($LASTEXITCODE -ne 0) { throw 'Update packaging failed' }
    & .\target\release\freshen-release.exe sign $publisherPrivate "$output\feed\manifest.json" "$output\feed\manifest.json.sig"
    if ($LASTEXITCODE -ne 0) { throw 'Update signing failed' }
    Write-Output "Initial installer: $output\installers\FreshenNativeTest-1.0.0-x64.exe"
    Write-Output "Signed version 2 feed: $output\feed"
    Write-Output 'After installing version 1, run native-test-app/scripts/enable-update.ps1.'
} finally {
    $env:FRESHEN_UI_VERSION = $priorVersion
    $env:FRESHEN_UI_PUBLIC_KEY_FILE = $priorKey
    Pop-Location
}
