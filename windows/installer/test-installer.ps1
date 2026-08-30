#requires -Version 7.0

[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$tests = Join-Path $PSScriptRoot 'tests\InstallerCoordinator.Tests.ps1'
if (-not (Get-Module -ListAvailable -Name Pester)) {
    throw 'Pester is required to run installer tests.'
}
Invoke-Pester -Path $tests -EnableExit
