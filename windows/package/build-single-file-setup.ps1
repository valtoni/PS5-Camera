[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $ReleaseDirectory,
    [Parameter()]
    [string] $OutputDirectory = (Join-Path $PSScriptRoot '..\\..\\target\\ps5-camera-setup-v1')
)

$ErrorActionPreference = 'Stop'

$release = (Resolve-Path -LiteralPath $ReleaseDirectory).Path
$manifest = Join-Path $release 'release-manifest.json'
if (-not (Test-Path -LiteralPath $manifest -PathType Leaf)) {
    throw "Release manifest ausente: $manifest"
}

$env:PS5CAM_SETUP_PAYLOAD_DIR = $release
try {
    & cargo build --release --offline -p ps5cam-setup
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build falhou com código $LASTEXITCODE."
    }
}
finally {
    Remove-Item Env:PS5CAM_SETUP_PAYLOAD_DIR -ErrorAction SilentlyContinue
}

$destination = Join-Path $OutputDirectory 'PS5-Camera-Setup.exe'
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $PSScriptRoot '..\\..\\target\\release\\PS5-Camera-Setup.exe') -Destination $destination -Force

[pscustomobject]@{
    setup = (Resolve-Path -LiteralPath $destination).Path
    bytes = (Get-Item -LiteralPath $destination).Length
    sha256 = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
} | ConvertTo-Json -Depth 3
