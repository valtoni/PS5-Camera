$PackageRoot = Split-Path -Parent $PSScriptRoot
$Assembler = Join-Path $PackageRoot 'release-assembler.ps1'
$PowerShell = (Get-Command 'pwsh.exe' -ErrorAction Stop).Source
$TestRoot = Join-Path ([IO.Path]::GetTempPath()) ('ps5cam-release-pester-' + [guid]::NewGuid().ToString('N'))
$InputPath = Join-Path $TestRoot 'release-input.json'
$OutputPath = Join-Path $TestRoot 'release-output'

function Invoke-ReleasePlanner {
    param([string[]] $Arguments)
    $output = & $PowerShell -NoLogo -NoProfile -File $Assembler @Arguments 2>&1 | Out-String
    return [ordered]@{ exit_code = $LASTEXITCODE; output = $output; document = ($output | ConvertFrom-Json) }
}

Describe 'PS5 camera deterministic release assembler' {
    BeforeAll {
        New-Item -ItemType Directory -Path $TestRoot | Out-Null
        $firmwarePath = Join-Path $TestRoot 'synthetic-firmware.bin'
        $infPath = Join-Path $TestRoot 'synthetic-driver.inf'
        [IO.File]::WriteAllText($firmwarePath, 'synthetic test fixture only', [Text.UTF8Encoding]::new($false))
        [IO.File]::WriteAllText($infPath, 'synthetic test fixture only', [Text.UTF8Encoding]::new($false))
        $firmwareHash = (Get-FileHash -LiteralPath $firmwarePath -Algorithm SHA256).Hash.ToLowerInvariant()

        [ordered]@{
            schemaVersion = 1
            releaseVersion = '1.2.3'
            sourceRevision = ('a' * 40)
            sourceDateEpoch = 1700000000
            packageValidation = [ordered]@{
                infVerifPassed = $false
                osTargets = @('10_GE_X64')
            }
            artifacts = @(
                [ordered]@{
                    role = 'driver_inf'
                    path = $infPath
                    fileName = 'ps5cam-boot.inf'
                    sha256 = ('0' * 64)
                },
                [ordered]@{
                    role = 'authorized_firmware'
                    path = $firmwarePath
                    fileName = 'ps5cam-firmware.bin'
                    sha256 = $firmwareHash
                    authorization = [ordered]@{
                        status = 'pending'
                        cleanRoom = $true
                        redistributionAllowed = $false
                        license = ''
                        source = 'synthetic-test'
                        approvalReference = ''
                    }
                },
                [ordered]@{
                    role = 'license'
                    path = $firmwarePath
                    fileName = 'ps5cam-firmware.bin'
                    sha256 = $firmwareHash
                }
            )
        } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $InputPath -Encoding utf8NoBOM

        $script:EmptyPlan = Invoke-ReleasePlanner -Arguments @()
        $script:BlockedPlan = Invoke-ReleasePlanner -Arguments @('-InputManifest', $InputPath)
        $script:AssembleAttempt = Invoke-ReleasePlanner -Arguments @(
            '-InputManifest', $InputPath,
            '-OutputDirectory', $OutputPath,
            '-Assemble',
            '-ConfirmReleaseVersion', '1.2.3'
        )
    }

    AfterAll {
        if (Test-Path -LiteralPath $TestRoot) {
            Remove-Item -LiteralPath $TestRoot -Recurse -Force
        }
    }

    It 'is dry-run by default and lists every required release role' {
        $EmptyPlan.exit_code | Should Be 0
        $EmptyPlan.document.mode | Should Be 'dry_run'
        $EmptyPlan.document.status | Should Be 'blocked'
        @($EmptyPlan.document.requirements).Count | Should Be 8
        @($EmptyPlan.document.generated_files) -contains 'sbom.cdx.json' | Should Be $true
    }

    It 'rejects unverified package metadata and mismatched file hashes' {
        $codes = @($BlockedPlan.document.blockers | ForEach-Object code)
        $codes -contains 'infverif_not_confirmed' | Should Be $true
        $codes -contains 'os_targets_incomplete' | Should Be $true
        $codes -contains 'artifact_hash_mismatch' | Should Be $true
    }

    It 'rejects firmware without complete redistribution authorization' {
        @($BlockedPlan.document.blockers | ForEach-Object code) -contains 'firmware_not_authorized' | Should Be $true
    }

    It 'accepts only the exact pinned MIT reference and its notice as V1 firmware evidence' {
        $referenceFirmware = Join-Path $TestRoot '21.01-03.20.00.04-00.00.00.bin'
        $referenceNotice = Join-Path $TestRoot 'firmware-reference-MIT-LICENSE.txt'
        Copy-Item -LiteralPath (Join-Path $PackageRoot '..\..\firmware\reference\21.01-03.20.00.04-00.00.00.bin') -Destination $referenceFirmware
        Copy-Item -LiteralPath (Join-Path $PackageRoot '..\..\firmware\reference\LICENSE') -Destination $referenceNotice
        $document = Get-Content -LiteralPath $InputPath -Raw | ConvertFrom-Json
        $firmware = @($document.artifacts | Where-Object role -eq 'authorized_firmware')[0]
        $firmware.path = $referenceFirmware
        $firmware.fileName = '21.01-03.20.00.04-00.00.00.bin'
        $firmware.sha256 = (Get-FileHash -LiteralPath $referenceFirmware -Algorithm SHA256).Hash.ToLowerInvariant()
        $firmware.authorization = [ordered]@{
            status = 'approved'
            cleanRoom = $false
            redistributionAllowed = $true
            redistributionBasis = 'third_party_mit_reference'
            license = 'MIT'
            source = 'https://github.com/prosperodev/hdcamera'
            sourceCommit = '8773610978d5a4d91a6a6d8063d48a4f3afcfe5b'
            noticeFile = 'firmware-reference-MIT-LICENSE.txt'
            approvalReference = 'upstream-mit-license-2021-prosperodev'
        }
        $notice = @($document.artifacts | Where-Object role -eq 'license')[0]
        $notice.path = $referenceNotice
        $notice.fileName = 'firmware-reference-MIT-LICENSE.txt'
        $notice.sha256 = (Get-FileHash -LiteralPath $referenceNotice -Algorithm SHA256).Hash.ToLowerInvariant()
        $document | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $InputPath -Encoding utf8NoBOM

        $plan = Invoke-ReleasePlanner -Arguments @('-InputManifest', $InputPath)
        $codes = @($plan.document.blockers | ForEach-Object code)
        $codes -contains 'firmware_not_authorized' | Should Be $false
        $codes -contains 'pinned_reference_firmware_mismatch' | Should Be $false
        $codes -contains 'pinned_reference_notice_missing' | Should Be $false
    }

    It 'does not create release output while any blocker remains' {
        $AssembleAttempt.exit_code -eq 0 | Should Be $false
        $AssembleAttempt.document.status | Should Be 'blocked'
        Test-Path -LiteralPath $OutputPath | Should Be $false
    }

    It 'blocks duplicate release file names before assembly can overwrite output' {
        @($BlockedPlan.document.blockers | ForEach-Object code) -contains 'duplicate_release_filename' | Should Be $true
        Test-Path -LiteralPath $OutputPath | Should Be $false
    }

    It 'keeps required roles and blockers in deterministic order' {
        ($EmptyPlan.document.requirements | ForEach-Object role) -join ',' | Should Be 'driver_inf,signed_catalog,authorized_firmware,windows_service,diagnostic_cli,installer,installer_engine,license'
        $BlockedPlan.document.release_version | Should Be '1.2.3'
    }
}
