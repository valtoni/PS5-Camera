[CmdletBinding()]
param(
    [Parameter()]
    [string] $InputManifest,

    [Parameter()]
    [string] $OutputDirectory,

    [Parameter()]
    [switch] $Assemble,

    [Parameter()]
    [string] $ConfirmReleaseVersion,

    [Parameter()]
    [string] $ManifestCertificateThumbprint
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$SchemaVersion = 1
$ExpectedHardwareId = 'USB\VID_05A9&PID_0580'
$PinnedReferenceFirmwareSha256 = '10af1aee76fe0057a88db7ebf5f3ebf32430633effb93722be4cd0a9ed4fce54'
$PinnedReferenceFirmwareFileName = '21.01-03.20.00.04-00.00.00.bin'
$PinnedReferenceNoticeFileName = 'firmware-reference-MIT-LICENSE.txt'
$RequiredRoles = @(
    'driver_inf',
    'signed_catalog',
    'authorized_firmware',
    'windows_service',
    'diagnostic_cli',
    'installer',
    'installer_engine',
    'license'
)
$BinaryRoles = @('windows_service', 'diagnostic_cli')
$Blockers = [System.Collections.Generic.List[object]]::new()
$Requirements = [System.Collections.Generic.List[object]]::new()
$VerifiedArtifacts = [System.Collections.Generic.List[object]]::new()

function Add-Blocker {
    param([string] $Code, [string] $Message, [string] $Resolution)
    $Blockers.Add([ordered]@{ code = $Code; message = $Message; resolution = $Resolution })
}

function Find-SignTool {
    $command = Get-Command 'signtool.exe' -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($command) { return $command.Source }
    $kits = Join-Path ([Environment]::GetFolderPath('ProgramFilesX86')) 'Windows Kits\10'
    if (-not (Test-Path -LiteralPath $kits)) { return $null }
    return Get-Item -Path (Join-Path $kits 'bin\*\x64\signtool.exe'), (Join-Path $kits 'bin\*\x86\signtool.exe') -ErrorAction SilentlyContinue |
        Where-Object { -not $_.PSIsContainer } |
        Sort-Object FullName -Descending |
        Select-Object -First 1 -ExpandProperty FullName
}

function Resolve-InputPath {
    param([string] $ManifestDirectory, [string] $Path)
    if ([IO.Path]::IsPathRooted($Path)) { return [IO.Path]::GetFullPath($Path) }
    return [IO.Path]::GetFullPath((Join-Path $ManifestDirectory $Path))
}

function Test-InfScope {
    param([string] $Path)
    $content = Get-Content -LiteralPath $Path -Raw
    $ids = @([regex]::Matches($content, 'USB\\VID_[0-9A-F]{4}&PID_[0-9A-F]{4}', 'IgnoreCase') |
        ForEach-Object { $_.Value.ToUpperInvariant() } | Sort-Object -Unique)
    return $ids.Count -eq 1 -and $ids[0] -ceq $ExpectedHardwareId -and
        $content -match '(?im)^\s*Class\s*=\s*USBDevice\s*$' -and
        $content -match '(?im)^\s*Include\s*=\s*winusb\.inf\s*$' -and
        $content -match '(?im)^\s*CatalogFile\s*=\s*ps5cam-boot\.cat\s*$'
}

function Invoke-SignToolVerify {
    param([string] $SignTool, [string[]] $Arguments)
    $output = & $SignTool @Arguments 2>&1 | Out-String
    return [ordered]@{ passed = ($LASTEXITCODE -eq 0); output = $output.Trim() }
}

function Write-DeterministicJson {
    param([string] $Path, [object] $Value, [DateTimeOffset] $Timestamp)
    $json = ($Value | ConvertTo-Json -Depth 12).Replace("`r`n", "`n") + "`n"
    [IO.File]::WriteAllText($Path, $json, [Text.UTF8Encoding]::new($false))
    [IO.File]::SetLastWriteTimeUtc($Path, $Timestamp.UtcDateTime)
}

function Get-DeterministicUuid {
    param([string] $Seed)
    $digest = [Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($Seed))
    $bytes = [byte[]]::new(16)
    [Array]::Copy($digest, $bytes, 16)
    $bytes[7] = ($bytes[7] -band 0x0f) -bor 0x50
    $bytes[8] = ($bytes[8] -band 0x3f) -bor 0x80
    return [Guid]::new($bytes).ToString()
}

function Get-ManifestSigningCertificate {
    param([Parameter(Mandatory)][string] $Thumbprint)
    $normalized = $Thumbprint.Replace(' ', '').ToUpperInvariant()
    if ($normalized -notmatch '^[0-9A-F]{40}$') { throw 'Manifest certificate thumbprint must contain exactly 40 hexadecimal characters.' }
    $matches = @(Get-ChildItem -Path 'Cert:\LocalMachine\My', 'Cert:\CurrentUser\My' -ErrorAction SilentlyContinue |
        Where-Object { $_.Thumbprint -ceq $normalized -and $_.HasPrivateKey })
    if ($matches.Count -ne 1) { throw 'Manifest signing certificate was not found uniquely with an accessible private key.' }
    $matches[0]
}

function Write-DetachedManifestSignature {
    param(
        [Parameter(Mandatory)][string] $ManifestPath,
        [Parameter(Mandatory)][string] $SignaturePath,
        [Parameter(Mandatory)][Security.Cryptography.X509Certificates.X509Certificate2] $Certificate,
        [Parameter(Mandatory)][DateTimeOffset] $Timestamp
    )
    Add-Type -AssemblyName System.Security.Cryptography.Pkcs
    $content = [Security.Cryptography.Pkcs.ContentInfo]::new([IO.File]::ReadAllBytes($ManifestPath))
    $cms = [Security.Cryptography.Pkcs.SignedCms]::new($content, $true)
    $signer = [Security.Cryptography.Pkcs.CmsSigner]::new($Certificate)
    $signer.IncludeOption = [Security.Cryptography.X509Certificates.X509IncludeOption]::EndCertOnly
    $signer.DigestAlgorithm = [Security.Cryptography.Oid]::new('2.16.840.1.101.3.4.2.1')
    $cms.ComputeSignature($signer)
    [IO.File]::WriteAllBytes($SignaturePath, $cms.Encode())
    [IO.File]::SetLastWriteTimeUtc($SignaturePath, $Timestamp.UtcDateTime)
}

$SignTool = Find-SignTool
$Manifest = $null
$ManifestDirectory = $null
$ReleaseVersion = $null
$SourceRevision = $null
$SourceDateEpoch = $null

if ([string]::IsNullOrWhiteSpace($InputManifest)) {
    Add-Blocker 'input_manifest_required' 'No release input manifest was provided.' 'Create a manifest conforming to release-input.schema.json using only real, authorized artifacts.'
}
elseif (-not (Test-Path -LiteralPath $InputManifest -PathType Leaf)) {
    Add-Blocker 'input_manifest_missing' "Release input manifest was not found: $InputManifest" 'Pass an existing JSON manifest.'
}
else {
    try {
        $resolvedManifest = (Resolve-Path -LiteralPath $InputManifest).Path
        $ManifestDirectory = Split-Path -Parent $resolvedManifest
        $Manifest = Get-Content -LiteralPath $resolvedManifest -Raw | ConvertFrom-Json
        if ($Manifest.schemaVersion -ne 1) { Add-Blocker 'invalid_schema_version' 'schemaVersion must be 1.' 'Use release-input.schema.json.' }
        $ReleaseVersion = [string]$Manifest.releaseVersion
        if ($ReleaseVersion -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$') { Add-Blocker 'invalid_release_version' 'releaseVersion is not a supported semantic version.' 'Use a fixed semantic version.' }
        $SourceRevision = [string]$Manifest.sourceRevision
        if ($SourceRevision -notmatch '^[0-9a-fA-F]{40}$') { Add-Blocker 'invalid_source_revision' 'sourceRevision must be a full 40-hex Git revision.' 'Record the exact source revision used for every binary.' }
        try { $SourceDateEpoch = [long]$Manifest.sourceDateEpoch } catch { $SourceDateEpoch = -1 }
        if ($SourceDateEpoch -lt 0) { Add-Blocker 'invalid_source_date_epoch' 'sourceDateEpoch must be a non-negative integer.' 'Use a fixed build timestamp.' }
        if ($Manifest.packageValidation.infVerifPassed -ne $true) { Add-Blocker 'infverif_not_confirmed' 'The input does not confirm a successful InfVerif /w run.' 'Run the package pipeline with the WDK and record the successful result.' }
        $targets = @($Manifest.packageValidation.osTargets | Sort-Object -Unique)
        if ($targets.Count -ne 2 -or $targets[0] -cne '10_GE_ARM64' -or $targets[1] -cne '10_GE_X64') { Add-Blocker 'os_targets_incomplete' 'Both reviewed x64 and ARM64 catalog targets are required.' 'Validate 10_GE_X64 and 10_GE_ARM64 with Inf2Cat.' }
    }
    catch {
        Add-Blocker 'input_manifest_invalid' $_.Exception.Message 'Fix the JSON and validate it against release-input.schema.json.'
        $Manifest = $null
    }
}

$artifactsByRole = @{}
$releaseFileNames = @{}
if ($Manifest) {
    if ($Manifest.PSObject.Properties.Name -notcontains 'artifacts') {
        Add-Blocker 'artifacts_list_missing' 'The input manifest has no artifacts array.' 'Provide every role required by release-input.schema.json.'
        $manifestArtifacts = @()
    }
    else {
        $manifestArtifacts = @($Manifest.artifacts)
    }
    foreach ($artifact in $manifestArtifacts) {
        if ($artifact.PSObject.Properties.Name -notcontains 'role') {
            Add-Blocker 'artifact_role_missing' 'An artifact entry has no role.' 'Set a role defined by release-input.schema.json.'
            continue
        }
        $role = [string]$artifact.role
        if ($role -notin $RequiredRoles) {
            Add-Blocker 'unknown_artifact_role' "Unknown artifact role: $role" 'Use only roles defined by release-input.schema.json.'
            continue
        }
        if ($artifactsByRole.ContainsKey($role)) {
            Add-Blocker 'duplicate_artifact_role' "Artifact role appears more than once: $role" 'Provide exactly one artifact for each role.'
            continue
        }
        $artifactsByRole[$role] = $artifact
    }
}

foreach ($role in $RequiredRoles) {
    $requirement = [ordered]@{ role = $role; kind = $(if ($role -in $BinaryRoles) { 'binary' } else { 'file' }); status = 'missing'; path = $null; sha256 = $null }
    if (-not $artifactsByRole.ContainsKey($role)) {
        Add-Blocker 'required_artifact_missing' "Required release artifact is missing: $role" 'Build or obtain the real authorized artifact, then add it with its SHA-256.'
        $Requirements.Add($requirement)
        continue
    }
    $artifact = $artifactsByRole[$role]
    $artifactProperties = @($artifact.PSObject.Properties.Name)
    $missingProperties = @(@('path', 'fileName', 'sha256') | Where-Object { $_ -notin $artifactProperties })
    if ($missingProperties.Count -gt 0) {
        Add-Blocker 'artifact_metadata_incomplete' "Artifact $role lacks: $($missingProperties -join ', ')." 'Provide path, fileName and SHA-256 for the real file.'
        $Requirements.Add($requirement)
        continue
    }
    $fileName = [string]$artifact.fileName
    if ([string]::IsNullOrWhiteSpace($fileName) -or $fileName -ne [IO.Path]::GetFileName($fileName)) {
        Add-Blocker 'unsafe_release_filename' "Artifact $role has an unsafe fileName." 'Use a plain file name without directories.'
        $Requirements.Add($requirement)
        continue
    }
    if ($releaseFileNames.ContainsKey($fileName)) {
        Add-Blocker 'duplicate_release_filename' "Artifacts $($releaseFileNames[$fileName]) and $role use the same fileName: $fileName" 'Give every release artifact a unique fileName; the assembler never overwrites output.'
        $Requirements.Add($requirement)
        continue
    }
    $releaseFileNames[$fileName] = $role
    $path = Resolve-InputPath -ManifestDirectory $ManifestDirectory -Path ([string]$artifact.path)
    $requirement.path = $path
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        Add-Blocker 'artifact_file_missing' "Artifact file is missing for $role`: $path" 'Provide the actual built file.'
        $Requirements.Add($requirement)
        continue
    }
    $declaredHash = ([string]$artifact.sha256).ToLowerInvariant()
    $actualHash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    $requirement.sha256 = $actualHash
    if ($declaredHash -notmatch '^[0-9a-f]{64}$' -or $actualHash -cne $declaredHash) {
        Add-Blocker 'artifact_hash_mismatch' "SHA-256 mismatch for $role." 'Rebuild from the recorded revision or update the manifest after reviewing the exact file.'
        $Requirements.Add($requirement)
        continue
    }
    $artifactVersion = if ($artifactProperties -contains 'version') { [string]$artifact.version } else { $null }
    if ($role -in $BinaryRoles -and $artifactVersion -cne $ReleaseVersion) {
        Add-Blocker 'binary_version_mismatch' "Binary $role does not declare release version $ReleaseVersion." 'Build every shipped binary from the same release version.'
    }
    if ($role -eq 'driver_inf' -and -not (Test-InfScope -Path $path)) {
        Add-Blocker 'driver_inf_scope_invalid' 'The release INF is not a catalog-backed WinUSB package scoped only to the boot HardwareId.' 'Use the staged INF generated by the reviewed package pipeline.'
    }
    if ($role -eq 'authorized_firmware') {
        $auth = if ($artifactProperties -contains 'authorization') { $artifact.authorization } else { $null }
        $isCleanRoom = $null -ne $auth -and $auth.cleanRoom -eq $true
        $isPinnedMitReference = $null -ne $auth -and $auth.cleanRoom -eq $false -and
            $auth.redistributionBasis -ceq 'third_party_mit_reference' -and
            $auth.license -ceq 'MIT' -and
            $auth.sourceCommit -ceq '8773610978d5a4d91a6a6d8063d48a4f3afcfe5b' -and
            $auth.noticeFile -ceq $PinnedReferenceNoticeFileName
        if ($null -eq $auth -or $auth.status -cne 'approved' -or $auth.redistributionAllowed -ne $true -or
            [string]::IsNullOrWhiteSpace([string]$auth.license) -or [string]::IsNullOrWhiteSpace([string]$auth.source) -or
            [string]::IsNullOrWhiteSpace([string]$auth.approvalReference) -or (-not $isCleanRoom -and -not $isPinnedMitReference)) {
            Add-Blocker 'firmware_not_authorized' 'Firmware lacks approved clean-room authorization or the pinned MIT reference provenance.' 'For clean-room firmware record its authorization; for the V1 reference use the exact MIT source commit, notice file and redistribution basis.'
        }
        if ($isPinnedMitReference -and ($fileName -cne $PinnedReferenceFirmwareFileName -or $actualHash -cne $PinnedReferenceFirmwareSha256)) {
            Add-Blocker 'pinned_reference_firmware_mismatch' 'The V1 MIT reference must be the exact reviewed firmware file and SHA-256.' 'Use 21.01-03.20.00.04-00.00.00.bin with SHA-256 10af1aee76fe0057a88db7ebf5f3ebf32430633effb93722be4cd0a9ed4fce54.'
        }
    }
    $requirement.status = 'verified'
    $Requirements.Add($requirement)
    $VerifiedArtifacts.Add([ordered]@{
        role = $role
        file_name = $fileName
        source_path = $path
        size = (Get-Item -LiteralPath $path).Length
        sha256 = $actualHash
        version = $artifactVersion
        authorization = $(if ($artifactProperties -contains 'authorization') { $artifact.authorization } else { $null })
    })
}

if ($artifactsByRole.ContainsKey('authorized_firmware')) {
    $firmwareArtifact = $artifactsByRole['authorized_firmware']
    $firmwareAuth = if (@($firmwareArtifact.PSObject.Properties.Name) -contains 'authorization') { $firmwareArtifact.authorization } else { $null }
    $isPinnedMitReference = $null -ne $firmwareAuth -and $firmwareAuth.cleanRoom -eq $false -and
        $firmwareAuth.redistributionBasis -ceq 'third_party_mit_reference' -and
        $firmwareAuth.license -ceq 'MIT' -and
        $firmwareAuth.sourceCommit -ceq '8773610978d5a4d91a6a6d8063d48a4f3afcfe5b' -and
        $firmwareAuth.noticeFile -ceq $PinnedReferenceNoticeFileName
    if ($isPinnedMitReference) {
        if (-not $artifactsByRole.ContainsKey('license') -or [string]$artifactsByRole['license'].fileName -cne $PinnedReferenceNoticeFileName) {
            Add-Blocker 'pinned_reference_notice_missing' 'The V1 MIT reference release must ship its exact MIT notice under the recorded notice file name.' "Add the license artifact as $PinnedReferenceNoticeFileName."
        }
    }
}

if ($artifactsByRole.ContainsKey('signed_catalog') -and $artifactsByRole.ContainsKey('driver_inf')) {
    if (-not $SignTool) {
        Add-Blocker 'signtool_missing' 'SignTool is unavailable, so catalog trust cannot be verified.' 'Install the supported Windows SDK/WDK and rerun the planner.'
    }
    else {
        $catalogPath = Resolve-InputPath $ManifestDirectory ([string]$artifactsByRole.signed_catalog.path)
        $infPath = Resolve-InputPath $ManifestDirectory ([string]$artifactsByRole.driver_inf.path)
        if ((Test-Path -LiteralPath $catalogPath -PathType Leaf) -and (Test-Path -LiteralPath $infPath -PathType Leaf)) {
            $signature = Invoke-SignToolVerify $SignTool @('verify', '/v', '/pa', $catalogPath)
            if (-not $signature.passed) { Add-Blocker 'catalog_signature_invalid' 'The catalog does not have a trusted PnP signature.' 'Sign through the authorized pipeline and establish the required trust chain.' }
            $membership = Invoke-SignToolVerify $SignTool @('verify', '/v', '/pa', '/c', $catalogPath, $infPath)
            if (-not $membership.passed) { Add-Blocker 'catalog_membership_invalid' 'The signed catalog does not validate the supplied INF.' 'Generate the CAT from this exact staged INF, then sign it.' }
        }
    }
}

if ($Assemble) {
    if ([string]::IsNullOrWhiteSpace($OutputDirectory)) { Add-Blocker 'output_directory_required' 'Assembly requires an explicit output directory.' 'Pass a new or empty -OutputDirectory.' }
    if ($ConfirmReleaseVersion -cne $ReleaseVersion) { Add-Blocker 'release_confirmation_required' "Assembly requires exact confirmation of release version $ReleaseVersion." "Pass -ConfirmReleaseVersion '$ReleaseVersion'." }
    if ([string]::IsNullOrWhiteSpace($ManifestCertificateThumbprint)) {
        Add-Blocker 'manifest_signing_certificate_required' 'Assembly requires an explicit development signing certificate for release-manifest.p7s.' 'Pass -ManifestCertificateThumbprint for the reviewed local development certificate.'
    }
    else {
        try { $script:ManifestSigningCertificate = Get-ManifestSigningCertificate $ManifestCertificateThumbprint }
        catch { Add-Blocker 'manifest_signing_certificate_invalid' $_.Exception.Message 'Use an accessible local development signing certificate with its private key.' }
    }
    if (-not [string]::IsNullOrWhiteSpace($OutputDirectory) -and (Test-Path -LiteralPath $OutputDirectory)) {
        if (@(Get-ChildItem -LiteralPath $OutputDirectory -Force).Count -ne 0) { Add-Blocker 'output_directory_not_empty' 'The assembler never overwrites release output.' 'Pass a new or empty output directory.' }
    }
}

$plan = [ordered]@{
    schema_version = $SchemaVersion
    status = $(if ($Blockers.Count) { 'blocked' } else { 'ready' })
    mode = $(if ($Assemble) { 'assemble' } else { 'dry_run' })
    release_version = $ReleaseVersion
    source_revision = $SourceRevision
    hardware_id = $ExpectedHardwareId
    signtool = $SignTool
    requirements = @($Requirements)
    blockers = @($Blockers)
    generated_files = @('release-manifest.json', 'release-manifest.p7s', 'sbom.cdx.json')
}

if (-not $Assemble) { $plan | ConvertTo-Json -Depth 10; exit 0 }
if ($Blockers.Count) { [Console]::Error.WriteLine(($plan | ConvertTo-Json -Depth 10)); exit 2 }

$output = [IO.Path]::GetFullPath($OutputDirectory)
if (-not (Test-Path -LiteralPath $output)) { New-Item -ItemType Directory -Path $output | Out-Null }
$timestamp = [DateTimeOffset]::FromUnixTimeSeconds($SourceDateEpoch)
$releaseArtifacts = [System.Collections.Generic.List[object]]::new()
foreach ($artifact in ($VerifiedArtifacts | Sort-Object role)) {
    $destination = Join-Path $output $artifact.file_name
    Copy-Item -LiteralPath $artifact.source_path -Destination $destination
    [IO.File]::SetLastWriteTimeUtc($destination, $timestamp.UtcDateTime)
    $releaseArtifacts.Add([ordered]@{ role = $artifact.role; file_name = $artifact.file_name; size = $artifact.size; sha256 = $artifact.sha256; version = $artifact.version })
}

$seed = $ReleaseVersion + ':' + $SourceRevision + ':' + (($releaseArtifacts | ForEach-Object { $_.sha256 }) -join ':')
$components = @($releaseArtifacts | ForEach-Object {
    [ordered]@{
        type = 'file'
        'bom-ref' = "urn:ps5cam:$($_.role):$($_.sha256)"
        name = $_.file_name
        version = $(if ($_.version) { $_.version } else { $ReleaseVersion })
        hashes = @([ordered]@{ alg = 'SHA-256'; content = $_.sha256 })
        properties = @([ordered]@{ name = 'org.ps5camera.release.role'; value = $_.role })
    }
})
$sbom = [ordered]@{
    bomFormat = 'CycloneDX'
    specVersion = '1.6'
    serialNumber = 'urn:uuid:' + (Get-DeterministicUuid $seed)
    version = 1
    metadata = [ordered]@{
        timestamp = $timestamp.UtcDateTime.ToString('yyyy-MM-ddTHH:mm:ssZ')
        tools = [ordered]@{ components = @([ordered]@{ type = 'application'; name = 'ps5cam-release-assembler'; version = '0.1.0' }) }
        component = [ordered]@{ type = 'application'; 'bom-ref' = 'urn:ps5cam:windows-release'; name = 'PS5 Camera Windows Driver'; version = $ReleaseVersion }
        properties = @([ordered]@{ name = 'vcs.revision'; value = $SourceRevision })
    }
    components = $components
    dependencies = @([ordered]@{ ref = 'urn:ps5cam:windows-release'; dependsOn = @($components | ForEach-Object { $_.'bom-ref' }) })
}
$sbomPath = Join-Path $output 'sbom.cdx.json'
Write-DeterministicJson $sbomPath $sbom $timestamp
$releaseManifest = [ordered]@{
    schema_version = 1
    release_version = $ReleaseVersion
    source_revision = $SourceRevision.ToLowerInvariant()
    source_date_epoch = $SourceDateEpoch
    hardware_ids = @($ExpectedHardwareId)
    artifacts = @($releaseArtifacts)
    firmware_authorization = $(
        $firmware = @($VerifiedArtifacts | Where-Object { $_.role -eq 'authorized_firmware' })
        if ($firmware.Count -eq 1) {
            $auth = $firmware[0].authorization
            [ordered]@{
                status = [string]$auth.status
                clean_room = $auth.cleanRoom -eq $true
                redistribution_allowed = $auth.redistributionAllowed -eq $true
                redistribution_basis = [string]$auth.redistributionBasis
                license = [string]$auth.license
                source = [string]$auth.source
                source_commit = [string]$auth.sourceCommit
                notice_file = [string]$auth.noticeFile
                approval_reference = [string]$auth.approvalReference
            }
        }
        else { $null }
    )
    sbom = [ordered]@{ file_name = 'sbom.cdx.json'; sha256 = (Get-FileHash $sbomPath -Algorithm SHA256).Hash.ToLowerInvariant() }
}
$manifestPath = Join-Path $output 'release-manifest.json'
Write-DeterministicJson $manifestPath $releaseManifest $timestamp
$signaturePath = Join-Path $output 'release-manifest.p7s'
Write-DetachedManifestSignature -ManifestPath $manifestPath -SignaturePath $signaturePath -Certificate $script:ManifestSigningCertificate -Timestamp $timestamp
$plan.status = 'completed'
$plan.generated_files = @($manifestPath, $signaturePath, $sbomPath)
$plan | ConvertTo-Json -Depth 10
