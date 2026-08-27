[CmdletBinding()]
param(
    [Parameter(Mandatory = $false)]
    [string]$MakensisPath,

    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

$repositoryRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$version = (Get-Content -Raw (Join-Path $repositoryRoot "VERSION")).Trim()
$versionMatch = [regex]::Match($version, "^(?<major>0|[1-9]\d*)\.(?<minor>0|[1-9]\d*)\.(?<patch>0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$")
if (-not $versionMatch.Success) {
    throw "VERSION must use SemVer. Found '$version'."
}

if (-not $MakensisPath) {
    $makensisCommand = Get-Command makensis.exe -ErrorAction SilentlyContinue
    if (-not $makensisCommand) {
        throw "NSIS 3.x is required. Install NSIS or pass -MakensisPath with the full path to makensis.exe."
    }
    $MakensisPath = $makensisCommand.Source
}

if (-not (Test-Path -LiteralPath $MakensisPath -PathType Leaf)) {
    throw "makensis.exe was not found at '$MakensisPath'."
}

$targetTriple = "x86_64-pc-windows-msvc"
if (-not $SkipBuild) {
    & cargo build -p wechat-send-guard --release --target $targetTriple
    if ($LASTEXITCODE -ne 0) {
        throw "Rust release build failed."
    }
}

$applicationPath = Join-Path $repositoryRoot "target\$targetTriple\release\WeChatSendGuard.exe"
if (-not (Test-Path -LiteralPath $applicationPath -PathType Leaf)) {
    throw "Release executable was not found at '$applicationPath'."
}

$outputDirectory = Join-Path $repositoryRoot "dist\windows"
New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null
$installerPath = Join-Path $outputDirectory "WeChatSendGuard-Setup-$version.exe"
$installerScript = Join-Path $PSScriptRoot "WeChatSendGuard.nsi"
$windowsVersion = "{0}.{1}.{2}.0" -f $versionMatch.Groups["major"].Value, $versionMatch.Groups["minor"].Value, $versionMatch.Groups["patch"].Value

& $MakensisPath "/V2" "/DAPP_VERSION=$version" "/DAPP_VERSION_WIN=$windowsVersion" "/DAPP_EXECUTABLE=$applicationPath" "/DOUTPUT_FILE=$installerPath" $installerScript
if ($LASTEXITCODE -ne 0) {
    throw "NSIS installer build failed."
}

& (Join-Path $PSScriptRoot "verify-release.ps1") -InstallerPath $installerPath
if ($LASTEXITCODE -ne 0) {
    throw "Release verification failed."
}

Get-Item -LiteralPath $installerPath | Select-Object FullName, Length, LastWriteTime
