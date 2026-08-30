[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$PowerShell = (Get-Process -Id $PID).Path
$Validator = Join-Path $PSScriptRoot 'validate-package.ps1'
$TestRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('ps5cam-package-test-' + [guid]::NewGuid().ToString('N'))
$ForbiddenProductId = '05' + '8C'
$ForbiddenDriverName = 'usb' + 'video'

function Invoke-Validation {
    param(
        [Parameter(Mandatory)][string] $Root,
        [Parameter(Mandatory)][int] $ExpectedExitCode,
        [Parameter(Mandatory)][string] $Scenario
    )

    $output = & $PowerShell -NoLogo -NoProfile -File $Validator -PackageRoot $Root 2>&1 | Out-String
    if ($LASTEXITCODE -ne $ExpectedExitCode) {
        throw "Scenario '$Scenario' expected exit code $ExpectedExitCode, got $LASTEXITCODE.`n$output"
    }
}

try {
    Invoke-Validation -Root $PSScriptRoot -ExpectedExitCode 0 -Scenario 'valid package'

    New-Item -ItemType Directory -Path $TestRoot | Out-Null
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'ps5cam-boot.inf') -Destination $TestRoot
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'package-manifest.json') -Destination $TestRoot

    $MutatedInf = Join-Path $TestRoot 'ps5cam-boot.inf'
    $OriginalInf = Get-Content -LiteralPath $MutatedInf -Raw

    $ForbiddenPidInf = $OriginalInf.Replace('PID_0580', "PID_$ForbiddenProductId")
    Set-Content -LiteralPath $MutatedInf -Value $ForbiddenPidInf -Encoding utf8NoBOM
    Invoke-Validation -Root $TestRoot -ExpectedExitCode 1 -Scenario 'forbidden final-mode hardware ID'

    $ForbiddenDriverInf = $OriginalInf.Replace('winusb.inf', "$ForbiddenDriverName.inf")
    Set-Content -LiteralPath $MutatedInf -Value $ForbiddenDriverInf -Encoding utf8NoBOM
    Invoke-Validation -Root $TestRoot -ExpectedExitCode 1 -Scenario 'forbidden camera function driver'

    $WrongClassInf = $OriginalInf.Replace('Class       = USBDevice', 'Class       = Camera')
    Set-Content -LiteralPath $MutatedInf -Value $WrongClassInf -Encoding utf8NoBOM
    Invoke-Validation -Root $TestRoot -ExpectedExitCode 1 -Scenario 'camera setup class'

    [ordered]@{
        schema_version = 1
        status = 'ok'
        scenarios = 4
    } | ConvertTo-Json
}
finally {
    if (Test-Path -LiteralPath $TestRoot) {
        Remove-Item -LiteralPath $TestRoot -Recurse -Force
    }
}
