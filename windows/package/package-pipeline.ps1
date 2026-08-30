[CmdletBinding()]
param(
    [Parameter()]
    [ValidateSet('Inspect', 'Catalog', 'TestSign', 'Install', 'Uninstall', 'Rollback')]
    [string] $Action = 'Inspect',

    [Parameter()]
    [switch] $Execute,

    [Parameter()]
    [string] $StagingDirectory,

    [Parameter()]
    [string] $StatePath,

    [Parameter()]
    [string] $ConfirmHardwareId,

    [Parameter()]
    [string[]] $OsTargets = @('10_GE_X64', '10_GE_ARM64'),

    [Parameter()]
    [string] $CertificateThumbprint,

    [Parameter()]
    [ValidatePattern('^[A-Za-z0-9_-]+$')]
    [string] $CertificateStore = 'My',

    [Parameter()]
    [switch] $MachineCertificateStore,

    [Parameter()]
    [string] $TimestampUrl,

    # GitHub-hosted releases use the pinned V1 development signer. Its root is
    # intentionally not imported into Root (that Windows action is interactive).
    # This switch verifies the exact catalog signature without changing trust.
    [Parameter()]
    [switch] $AllowUntrustedDevelopmentSigner
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$SchemaVersion = 1
$ExpectedHardwareId = 'USB\VID_05A9&PID_0580'
$PackageId = 'org.ps5camera.boot.winusb'
$InfName = 'ps5cam-boot.inf'
$CatalogName = 'ps5cam-boot.cat'
$DevelopmentCertificateThumbprint = 'EDAF55A1E4AE0C8C197988F7286626BD51228CA2'
$PackageRoot = $PSScriptRoot
$Validator = Join-Path $PackageRoot 'validate-package.ps1'
$PowerShellExe = (Get-Process -Id $PID).Path
$Blockers = [System.Collections.Generic.List[object]]::new()
$Steps = [System.Collections.Generic.List[object]]::new()

function Add-Blocker {
    param(
        [Parameter(Mandatory)][string] $Code,
        [Parameter(Mandatory)][string] $Message,
        [Parameter(Mandatory)][string] $Resolution
    )
    $Blockers.Add([ordered]@{
        code = $Code
        message = $Message
        resolution = $Resolution
    })
}

function Add-Step {
    param(
        [Parameter(Mandatory)][string] $Name,
        [Parameter(Mandatory)][string] $Command,
        [Parameter(Mandatory)][string[]] $Arguments,
        [Parameter(Mandatory)][bool] $MutatesSystem
    )
    $Steps.Add([ordered]@{
        name = $Name
        command = $Command
        arguments = @($Arguments)
        mutates_system = $MutatesSystem
    })
}

function Find-WindowsTool {
    param([Parameter(Mandatory)][string] $Name)

    $command = Get-Command $Name -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -ne $command) {
        return $command.Source
    }

    $searchRoots = [System.Collections.Generic.List[string]]::new()
    if (-not [string]::IsNullOrWhiteSpace($env:WindowsSdkDir)) {
        $searchRoots.Add($env:WindowsSdkDir)
    }
    $programFilesX86 = [Environment]::GetFolderPath('ProgramFilesX86')
    if (-not [string]::IsNullOrWhiteSpace($programFilesX86)) {
        $kitsRoot = Join-Path $programFilesX86 'Windows Kits\10'
        if (Test-Path -LiteralPath $kitsRoot -PathType Container) {
            $searchRoots.Add($kitsRoot)
        }
    }

    foreach ($root in ($searchRoots | Select-Object -Unique)) {
        $candidatePatterns = @(
            (Join-Path $root "bin\*\x64\$Name"),
            (Join-Path $root "bin\*\x86\$Name"),
            (Join-Path $root "bin\x64\$Name"),
            (Join-Path $root "bin\x86\$Name"),
            (Join-Path $root "Tools\*\x64\$Name"),
            (Join-Path $root "Tools\x64\$Name")
        )
        $match = Get-Item -Path $candidatePatterns -ErrorAction SilentlyContinue |
            Where-Object { -not $_.PSIsContainer } |
            Sort-Object FullName -Descending |
            Select-Object -First 1
        if ($null -ne $match) {
            return $match.FullName
        }
    }
    return $null
}

function Invoke-NativeTool {
    param(
        [Parameter(Mandatory)][string] $Tool,
        [Parameter(Mandatory)][string[]] $Arguments
    )

    $output = & $Tool @Arguments 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed with exit code $LASTEXITCODE`: $Tool $($Arguments -join ' ')`n$output"
    }
    return $output
}

function Assert-ExpectedCatalogSignature {
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][string] $ExpectedThumbprint
    )

    # SignedCms.CheckSignature($true) validates the exact catalog signature
    # while deliberately skipping certificate-chain policy. Unlike adding a
    # root certificate, it never prompts or changes machine/user trust state.
    Add-Type -AssemblyName System.Security.Cryptography.Pkcs
    $cms = [Security.Cryptography.Pkcs.SignedCms]::new()
    $cms.Decode([IO.File]::ReadAllBytes($Path))
    $cms.CheckSignature($true)
    if ($cms.SignerInfos.Count -ne 1 -or $null -eq $cms.SignerInfos[0].Certificate) {
        throw 'Catalog must contain exactly one embedded signing certificate.'
    }
    $actualThumbprint = $cms.SignerInfos[0].Certificate.Thumbprint.Replace(' ', '').ToUpperInvariant()
    $expected = $ExpectedThumbprint.Replace(' ', '').ToUpperInvariant()
    if ($actualThumbprint -cne $expected) {
        throw "Catalog signature is not cryptographically valid for the pinned development signer. signer=$actualThumbprint"
    }
}

function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Assert-InfScope {
    param([Parameter(Mandatory)][string] $InfPath)

    if (-not (Test-Path -LiteralPath $InfPath -PathType Leaf)) {
        throw "Staged INF is missing: $InfPath"
    }
    $content = Get-Content -LiteralPath $InfPath -Raw
    $hardwareIds = @([regex]::Matches($content, 'USB\\VID_[0-9A-F]{4}&PID_[0-9A-F]{4}', 'IgnoreCase') |
        ForEach-Object { $_.Value.ToUpperInvariant() } |
        Sort-Object -Unique)
    if ($hardwareIds.Count -ne 1 -or $hardwareIds[0] -cne $ExpectedHardwareId) {
        throw "Staged INF scope violation: only $ExpectedHardwareId is allowed."
    }
    if ($content -notmatch '(?im)^\s*Class\s*=\s*USBDevice\s*$') {
        throw 'Staged INF must use the USBDevice setup class.'
    }
    if ($content -notmatch '(?im)^\s*Include\s*=\s*winusb\.inf\s*$' -or
        $content -notmatch '(?im)^\s*Needs\s*=\s*WINUSB\.NT\s*$') {
        throw 'Staged INF must use the Windows inbox WinUSB installation sections.'
    }
    if ($content -match '(?im)^\s*(AddService|CopyFiles|CoInstallers32)\s*=') {
        throw 'Staged INF must not install a custom service, binary, or co-installer.'
    }
}

function Get-NormalizedPath {
    param([Parameter(Mandatory)][string] $Path)
    return [IO.Path]::GetFullPath($Path)
}

function Read-TransactionState {
    param([Parameter(Mandatory)][string] $Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Transaction state is missing: $Path"
    }
    $state = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    if ($state.schema_version -ne $SchemaVersion -or
        $state.package_id -cne $PackageId -or
        $state.hardware_id -cne $ExpectedHardwareId -or
        $state.original_inf_name -cne $InfName) {
        throw 'Transaction state does not belong to this package and hardware ID.'
    }
    if ($state.published_name -notmatch '^oem\d+\.inf$') {
        throw 'Transaction state does not contain a safe published driver name.'
    }
    return $state
}

function Write-TransactionState {
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][object] $State
    )
    $State | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $Path -Encoding utf8NoBOM
}

function Write-ExecutionError {
    param([Parameter(Mandatory)][string] $Message)
    $document = [ordered]@{
        schema_version = $SchemaVersion
        status = 'error'
        action = $Action.ToLowerInvariant()
        error = [ordered]@{
            code = 'execution_failed'
            message = $Message
        }
    } | ConvertTo-Json -Depth 6
    [Console]::Error.WriteLine($document)
}

$Tools = [ordered]@{
    infverif = Find-WindowsTool -Name 'infverif.exe'
    inf2cat = Find-WindowsTool -Name 'inf2cat.exe'
    signtool = Find-WindowsTool -Name 'signtool.exe'
    pnputil = Find-WindowsTool -Name 'pnputil.exe'
}

try {
    $null = Invoke-NativeTool -Tool $PowerShellExe -Arguments @(
        '-NoLogo', '-NoProfile', '-File', $Validator, '-SkipWdk'
    )
}
catch {
    Add-Blocker -Code 'source_package_invalid' -Message $_.Exception.Message -Resolution 'Fix the declarative source package before running the pipeline.'
}

$ActionKey = $Action.ToLowerInvariant()
$osTargetValue = $OsTargets -join ','
foreach ($target in $OsTargets) {
    if ($target -notmatch '^10_[A-Z0-9]+_(X64|ARM64)$') {
        Add-Blocker -Code 'invalid_os_target' -Message "Unsupported or unsafe Inf2Cat target: $target" -Resolution 'Use reviewed Inf2Cat identifiers such as 10_GE_X64 and 10_GE_ARM64.'
    }
}

$stagePath = if ([string]::IsNullOrWhiteSpace($StagingDirectory)) { '<required:StagingDirectory>' } else { Get-NormalizedPath -Path $StagingDirectory }
$stateFile = if ([string]::IsNullOrWhiteSpace($StatePath)) { '<required:StatePath>' } else { Get-NormalizedPath -Path $StatePath }
$stagedInf = if ($stagePath.StartsWith('<required:')) { "$stagePath\$InfName" } else { Join-Path $stagePath $InfName }
$stagedCatalog = if ($stagePath.StartsWith('<required:')) { "$stagePath\$CatalogName" } else { Join-Path $stagePath $CatalogName }

Add-Step -Name 'validate-source' -Command $PowerShellExe -Arguments @('-NoLogo', '-NoProfile', '-File', $Validator, '-SkipWdk') -MutatesSystem $false

switch ($Action) {
    'Inspect' {
        foreach ($toolName in @('infverif', 'inf2cat', 'signtool')) {
            if ([string]::IsNullOrWhiteSpace($Tools[$toolName])) {
                Add-Blocker -Code "wdk_$($toolName)_missing" -Message "$toolName is not available." -Resolution 'Install a supported Windows Driver Kit and Windows SDK, then rerun Inspect.'
            }
        }
    }
    'Catalog' {
        Add-Step -Name 'stage-package' -Command 'Copy-Item' -Arguments @($InfName, $stagePath) -MutatesSystem $false
        Add-Step -Name 'generate-catalog' -Command ($(if ($Tools.inf2cat) { $Tools.inf2cat } else { 'inf2cat.exe' })) -Arguments @("/driver:$stagePath", "/os:$osTargetValue", '/uselocaltime') -MutatesSystem $false
        Add-Step -Name 'verify-inf' -Command ($(if ($Tools.infverif) { $Tools.infverif } else { 'infverif.exe' })) -Arguments @('/w', $stagedInf) -MutatesSystem $false
        if ([string]::IsNullOrWhiteSpace($StagingDirectory)) {
            Add-Blocker -Code 'staging_directory_required' -Message 'Catalog requires an explicit staging directory.' -Resolution 'Pass -StagingDirectory with a new or empty directory.'
        }
        else {
            $normalizedStage = Get-NormalizedPath -Path $StagingDirectory
            if ($normalizedStage.TrimEnd('\') -ieq (Get-NormalizedPath -Path $PackageRoot).TrimEnd('\')) {
                Add-Blocker -Code 'unsafe_staging_directory' -Message 'StagingDirectory must not be the source package directory.' -Resolution 'Pass a separate new or empty directory.'
            }
            elseif (Test-Path -LiteralPath $normalizedStage -PathType Container) {
                $stageEntries = @(Get-ChildItem -LiteralPath $normalizedStage -Force)
                if ($stageEntries.Count -ne 0) {
                    Add-Blocker -Code 'staging_directory_not_empty' -Message 'Catalog never overwrites an existing staging directory.' -Resolution 'Pass a new or empty staging directory.'
                }
            }
        }
        if (-not $Tools.inf2cat) {
            Add-Blocker -Code 'wdk_inf2cat_missing' -Message 'Inf2Cat is not available; no catalog can be generated.' -Resolution 'Install a supported WDK containing Inf2Cat.'
        }
        if (-not $Tools.infverif) {
            Add-Blocker -Code 'wdk_infverif_missing' -Message 'InfVerif is not available; package isolation cannot be verified.' -Resolution 'Install a supported WDK containing InfVerif.'
        }
    }
    'TestSign' {
        $signArguments = @('sign', '/v', '/fd', 'SHA256', '/s', $CertificateStore)
        if ($MachineCertificateStore) { $signArguments += '/sm' }
        $signArguments += @('/sha1', $(if ($CertificateThumbprint) { $CertificateThumbprint } else { '<required:CertificateThumbprint>' }))
        if (-not [string]::IsNullOrWhiteSpace($TimestampUrl)) {
            $signArguments += @('/tr', $TimestampUrl, '/td', 'SHA256')
        }
        $signArguments += $stagedCatalog
        Add-Step -Name 'test-sign-catalog' -Command ($(if ($Tools.signtool) { $Tools.signtool } else { 'signtool.exe' })) -Arguments $signArguments -MutatesSystem $false
        if ($AllowUntrustedDevelopmentSigner) {
            Add-Step -Name 'verify-catalog-signature' -Command 'SignedCms.CheckSignature' -Arguments @($stagedCatalog, $CertificateThumbprint) -MutatesSystem $false
        }
        else {
            Add-Step -Name 'verify-catalog-signature' -Command ($(if ($Tools.signtool) { $Tools.signtool } else { 'signtool.exe' })) -Arguments @('verify', '/v', '/pa', $stagedCatalog) -MutatesSystem $false
        }
        if ([string]::IsNullOrWhiteSpace($StagingDirectory)) {
            Add-Blocker -Code 'staging_directory_required' -Message 'TestSign requires the catalog staging directory.' -Resolution 'Pass -StagingDirectory produced by Catalog.'
        }
        elseif (-not (Test-Path -LiteralPath $stagedInf -PathType Leaf) -or -not (Test-Path -LiteralPath $stagedCatalog -PathType Leaf)) {
            Add-Blocker -Code 'staged_package_missing' -Message 'TestSign requires both the staged INF and generated catalog.' -Resolution 'Run Catalog successfully and pass its StagingDirectory.'
        }
        if ($CertificateThumbprint -notmatch '^[0-9A-Fa-f]{40}$') {
            Add-Blocker -Code 'certificate_thumbprint_required' -Message 'A 40-hex certificate thumbprint is required.' -Resolution 'Provision an authorized test certificate and pass -CertificateThumbprint explicitly.'
        }
        elseif ($AllowUntrustedDevelopmentSigner -and $CertificateThumbprint.Replace(' ', '').ToUpperInvariant() -cne $DevelopmentCertificateThumbprint) {
            Add-Blocker -Code 'unexpected_development_signer' -Message 'Untrusted development verification is limited to the pinned V1 development signing certificate.' -Resolution "Use $DevelopmentCertificateThumbprint or omit -AllowUntrustedDevelopmentSigner."
        }
        if (-not [string]::IsNullOrWhiteSpace($TimestampUrl)) {
            $uri = $null
            if (-not [Uri]::TryCreate($TimestampUrl, [UriKind]::Absolute, [ref] $uri) -or $uri.Scheme -ne 'https') {
                Add-Blocker -Code 'invalid_timestamp_url' -Message 'TimestampUrl must be an absolute HTTPS URL.' -Resolution 'Use a reviewed RFC 3161 HTTPS timestamp endpoint or omit timestamping for offline test signing.'
            }
        }
        if (-not $Tools.signtool) {
            Add-Blocker -Code 'sdk_signtool_missing' -Message 'SignTool is not available; the catalog cannot be test-signed or verified.' -Resolution 'Install a supported Windows SDK/WDK containing SignTool.'
        }
    }
    'Install' {
        Add-Step -Name 'verify-catalog-signature' -Command ($(if ($Tools.signtool) { $Tools.signtool } else { 'signtool.exe' })) -Arguments @('verify', '/v', '/pa', $stagedCatalog) -MutatesSystem $false
        Add-Step -Name 'install-driver-package' -Command ($(if ($Tools.pnputil) { $Tools.pnputil } else { 'pnputil.exe' })) -Arguments @('/add-driver', $stagedInf, '/install') -MutatesSystem $true
        if ([string]::IsNullOrWhiteSpace($StagingDirectory)) {
            Add-Blocker -Code 'staging_directory_required' -Message 'Install requires a signed staging directory.' -Resolution 'Pass -StagingDirectory produced by Catalog and TestSign.'
        }
        elseif (-not (Test-Path -LiteralPath $stagedInf -PathType Leaf) -or -not (Test-Path -LiteralPath $stagedCatalog -PathType Leaf)) {
            Add-Blocker -Code 'staged_package_missing' -Message 'Install requires both the staged INF and signed catalog.' -Resolution 'Run Catalog and TestSign successfully before Install.'
        }
        if ([string]::IsNullOrWhiteSpace($StatePath)) {
            Add-Blocker -Code 'state_path_required' -Message 'Install requires an explicit transaction state path.' -Resolution 'Pass -StatePath in a controlled writable directory.'
        }
        elseif (Test-Path -LiteralPath $StatePath) {
            Add-Blocker -Code 'state_path_exists' -Message 'Install never overwrites existing transaction state.' -Resolution 'Choose a new StatePath or complete recovery using the existing state.'
        }
        else {
            $stateParentForPlan = Split-Path -Parent (Get-NormalizedPath -Path $StatePath)
            if ([string]::IsNullOrWhiteSpace($stateParentForPlan) -or -not (Test-Path -LiteralPath $stateParentForPlan -PathType Container)) {
                Add-Blocker -Code 'state_parent_missing' -Message 'StatePath parent directory does not exist.' -Resolution 'Create and secure the parent directory before installation.'
            }
        }
        if (-not $Tools.signtool) {
            Add-Blocker -Code 'sdk_signtool_missing' -Message 'SignTool is required to verify the catalog before installation.' -Resolution 'Install a supported Windows SDK/WDK containing SignTool.'
        }
        if (-not $Tools.pnputil) {
            Add-Blocker -Code 'pnputil_missing' -Message 'PnPUtil is not available.' -Resolution 'Run on a supported Windows installation.'
        }
    }
    { $_ -in @('Uninstall', 'Rollback') } {
        Add-Step -Name 'remove-driver-package' -Command ($(if ($Tools.pnputil) { $Tools.pnputil } else { 'pnputil.exe' })) -Arguments @('/delete-driver', '<published-name-from-state>', '/uninstall') -MutatesSystem $true
        if ($Action -eq 'Rollback') {
            Add-Step -Name 'rescan-devices' -Command ($(if ($Tools.pnputil) { $Tools.pnputil } else { 'pnputil.exe' })) -Arguments @('/scan-devices') -MutatesSystem $true
        }
        if ([string]::IsNullOrWhiteSpace($StatePath)) {
            Add-Blocker -Code 'state_path_required' -Message "$Action requires transaction state created by Install." -Resolution 'Pass the exact -StatePath emitted by the corresponding Install operation.'
        }
        elseif (Test-Path -LiteralPath $StatePath -PathType Leaf) {
            try {
                $stateForPlan = Read-TransactionState -Path $StatePath
                $Steps | Where-Object { $_.name -eq 'remove-driver-package' } | ForEach-Object {
                    $_.arguments[1] = $stateForPlan.published_name
                }
            }
            catch {
                Add-Blocker -Code 'invalid_transaction_state' -Message $_.Exception.Message -Resolution 'Use the unmodified state emitted by this package pipeline.'
            }
        }
        else {
            Add-Blocker -Code 'transaction_state_missing' -Message "Transaction state was not found: $StatePath" -Resolution 'Use the state path emitted by Install; never guess an oem#.inf name.'
        }
        if (-not $Tools.pnputil) {
            Add-Blocker -Code 'pnputil_missing' -Message 'PnPUtil is not available.' -Resolution 'Run on a supported Windows installation.'
        }
    }
}

if ($AllowUntrustedDevelopmentSigner -and $Action -ne 'TestSign') {
    Add-Blocker -Code 'untrusted_verification_scope_invalid' -Message 'AllowUntrustedDevelopmentSigner is permitted only for TestSign.' -Resolution 'Use the switch only in the ephemeral release signing step.'
}

if ($Execute -and $Action -ne 'Inspect') {
    if ($ConfirmHardwareId -cne $ExpectedHardwareId) {
        Add-Blocker -Code 'hardware_confirmation_required' -Message "Execution requires exact confirmation of $ExpectedHardwareId." -Resolution "Pass -ConfirmHardwareId '$ExpectedHardwareId'."
    }
    if (-not (Test-IsAdministrator)) {
        Add-Blocker -Code 'elevation_required' -Message 'Execution requires an elevated PowerShell process.' -Resolution 'Review the dry-run plan, then rerun from an Administrator terminal.'
    }
}

$result = [ordered]@{
    schema_version = $SchemaVersion
    status = $(if ($Blockers.Count -eq 0) { 'ready' } else { 'blocked' })
    action = $ActionKey
    mode = $(if ($Execute) { 'execute' } else { 'dry_run' })
    hardware_id = $ExpectedHardwareId
    tools = $Tools
    steps = @($Steps)
    blockers = @($Blockers)
}

if (-not $Execute -or $Action -eq 'Inspect') {
    $result | ConvertTo-Json -Depth 8
    exit 0
}

if ($Blockers.Count -gt 0) {
    [Console]::Error.WriteLine(($result | ConvertTo-Json -Depth 8))
    exit 2
}

try {
    switch ($Action) {
        'Catalog' {
            $resolvedStage = Get-NormalizedPath -Path $StagingDirectory
            if ($resolvedStage.TrimEnd('\') -ieq (Get-NormalizedPath -Path $PackageRoot).TrimEnd('\')) {
                throw 'StagingDirectory must not be the source package directory.'
            }
            if (Test-Path -LiteralPath $resolvedStage) {
                $entries = @(Get-ChildItem -LiteralPath $resolvedStage -Force)
                if ($entries.Count -ne 0) {
                    throw 'StagingDirectory must be new or empty; the pipeline never overwrites staging artifacts.'
                }
            }
            else {
                New-Item -ItemType Directory -Path $resolvedStage | Out-Null
            }
            $destinationInf = Join-Path $resolvedStage $InfName
            Copy-Item -LiteralPath (Join-Path $PackageRoot $InfName) -Destination $destinationInf
            Copy-Item -LiteralPath (Join-Path $PackageRoot 'package-manifest.json') -Destination $resolvedStage
            $content = Get-Content -LiteralPath $destinationInf -Raw
            $content = [regex]::Replace(
                $content,
                '(?m)^(Provider\s*=.*)$',
                { param($match) $match.Groups[1].Value + "`r`nCatalogFile = $CatalogName" },
                1
            )
            Set-Content -LiteralPath $destinationInf -Value $content -Encoding utf8NoBOM
            Assert-InfScope -InfPath $destinationInf
            $null = Invoke-NativeTool -Tool $Tools.inf2cat -Arguments @("/driver:$resolvedStage", "/os:$osTargetValue", '/uselocaltime')
            $generatedCatalog = Join-Path $resolvedStage $CatalogName
            if (-not (Test-Path -LiteralPath $generatedCatalog -PathType Leaf)) {
                throw "Inf2Cat completed without producing $generatedCatalog."
            }
            $null = Invoke-NativeTool -Tool $Tools.infverif -Arguments @('/w', $destinationInf)
            $result.status = 'completed'
            $result['artifacts'] = @($destinationInf, $generatedCatalog)
        }
        'TestSign' {
            Assert-InfScope -InfPath $stagedInf
            if (-not (Test-Path -LiteralPath $stagedCatalog -PathType Leaf)) {
                throw "Catalog is missing: $stagedCatalog"
            }
            $signStep = $Steps | Where-Object { $_.name -eq 'test-sign-catalog' } | Select-Object -First 1
            $null = Invoke-NativeTool -Tool $Tools.signtool -Arguments $signStep.arguments
            if ($AllowUntrustedDevelopmentSigner) {
                Assert-ExpectedCatalogSignature -Path $stagedCatalog -ExpectedThumbprint $CertificateThumbprint
            }
            else {
                $null = Invoke-NativeTool -Tool $Tools.signtool -Arguments @('verify', '/v', '/pa', $stagedCatalog)
            }
            $result.status = 'completed'
            $result['signed_catalog'] = $stagedCatalog
        }
        'Install' {
            Assert-InfScope -InfPath $stagedInf
            if (-not (Test-Path -LiteralPath $stagedCatalog -PathType Leaf)) {
                throw "Signed catalog is missing: $stagedCatalog"
            }
            $null = Invoke-NativeTool -Tool $Tools.signtool -Arguments @('verify', '/v', '/pa', $stagedCatalog)
            if (Test-Path -LiteralPath $StatePath) {
                throw 'StatePath already exists; refusing to overwrite transaction history.'
            }
            $stateParent = Split-Path -Parent $StatePath
            if ([string]::IsNullOrWhiteSpace($stateParent) -or -not (Test-Path -LiteralPath $stateParent -PathType Container)) {
                throw 'StatePath parent directory must already exist.'
            }
            $pendingState = [ordered]@{
                schema_version = $SchemaVersion
                package_id = $PackageId
                hardware_id = $ExpectedHardwareId
                original_inf_name = $InfName
                published_name = $null
                status = 'install_pending'
                inf_sha256 = (Get-FileHash -LiteralPath $stagedInf -Algorithm SHA256).Hash.ToLowerInvariant()
                catalog_sha256 = (Get-FileHash -LiteralPath $stagedCatalog -Algorithm SHA256).Hash.ToLowerInvariant()
            }
            Write-TransactionState -Path $StatePath -State $pendingState
            $installOutput = Invoke-NativeTool -Tool $Tools.pnputil -Arguments @('/add-driver', $stagedInf, '/install')
            $publishedNames = @([regex]::Matches($installOutput, '\boem\d+\.inf\b', 'IgnoreCase') | ForEach-Object { $_.Value.ToLowerInvariant() } | Sort-Object -Unique)
            if ($publishedNames.Count -ne 1) {
                throw 'PnPUtil succeeded but did not report exactly one published oem#.inf name; transaction remains install_pending for manual recovery.'
            }
            $pendingState.published_name = $publishedNames[0]
            $pendingState.status = 'installed'
            Write-TransactionState -Path $StatePath -State $pendingState
            $result.status = 'completed'
            $result['transaction_state'] = $StatePath
            $result['published_name'] = $publishedNames[0]
        }
        { $_ -in @('Uninstall', 'Rollback') } {
            $transaction = Read-TransactionState -Path $StatePath
            if ($transaction.status -cne 'installed') {
                throw "Transaction status must be installed, found '$($transaction.status)'."
            }
            $null = Invoke-NativeTool -Tool $Tools.pnputil -Arguments @('/delete-driver', $transaction.published_name, '/uninstall')
            if ($Action -eq 'Rollback') {
                $null = Invoke-NativeTool -Tool $Tools.pnputil -Arguments @('/scan-devices')
                $transaction.status = 'rolled_back'
            }
            else {
                $transaction.status = 'uninstalled'
            }
            Write-TransactionState -Path $StatePath -State $transaction
            $result.status = 'completed'
            $result['published_name'] = $transaction.published_name
            $result['transaction_state'] = $StatePath
        }
    }
    $result | ConvertTo-Json -Depth 8
}
catch {
    Write-ExecutionError -Message $_.Exception.Message
    exit 3
}
