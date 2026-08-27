[CmdletBinding()]
param(
    [Parameter(Mandatory = $false)]
    [string]$InstallerPath
)

$ErrorActionPreference = "Stop"

$repositoryRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$versionPath = Join-Path $repositoryRoot "VERSION"
$cargoPath = Join-Path $repositoryRoot "Cargo.toml"

$version = (Get-Content -Raw $versionPath).Trim()
if ($version -notmatch "^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$") {
    throw "VERSION must use SemVer. Found '$version'."
}

$cargo = Get-Content -Raw $cargoPath
if ($cargo -notmatch ('version\s*=\s*"' + [regex]::Escape($version) + '"')) {
    throw "Cargo workspace version does not match VERSION ($version)."
}

if ($InstallerPath) {
    $resolvedInstaller = Resolve-Path -LiteralPath $InstallerPath
    $size = (Get-Item -LiteralPath $resolvedInstaller).Length
    $limit = 15MB
    if ($size -gt $limit) {
        throw "Installer is $size bytes, above the 15 MB release target."
    }
    Write-Output "Installer size check passed: $size bytes."
}

Write-Output "Release metadata check passed for version $version."
