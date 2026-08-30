#requires -Version 5.1

[CmdletBinding()]
param(
    [Parameter()][ValidateSet('Install', 'Repair', 'Uninstall')][string] $Action = 'Install',
    [Parameter()][string] $ReleaseManifest,
    [Parameter()][string] $ConfirmReleaseVersion,
    [Parameter()][string] $ConfirmDevelopmentCertificateThumbprint,
    [Parameter()][switch] $RemoveDevelopmentCertificate,
    [Parameter()][switch] $EmbeddedPayload,
    [Parameter()][switch] $Execute
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$HardwareId = 'USB\VID_05A9&PID_0580'
$ServiceName = 'PS5CameraService'
$EventSource = 'PS5CameraService'
$ProductName = 'PS5 Camera'
$ExpectedCertificateThumbprint = 'EDAF55A1E4AE0C8C197988F7286626BD51228CA2'
$ExpectedCertificateDerBase64 = 'MIIEIjCCAoqgAwIBAgIQHXOrH2sfEoFOIHQtB7K9sDANBgkqhkiG9w0BAQsFADApMScwJQYDVQQDDB5QUzUgQ2FtZXJhIERldmVsb3BtZW50IFNpZ25pbmcwHhcNMjYwODI5MDIxMzUzWhcNMjgwODI5MDIyMzUzWjApMScwJQYDVQQDDB5QUzUgQ2FtZXJhIERldmVsb3BtZW50IFNpZ25pbmcwggGiMA0GCSqGSIb3DQEBAQUAA4IBjwAwggGKAoIBgQC9kvPSr50223y1qqc6SjtWqlo5oI5CBrou7AG02SMEkhaDBH0VqeZOzIK2Gg7sxpISRikhMrapUacJyxaRuPWgbaAXiibSmVCAqRGG1EAJ89DlN+JT7UdTfiMdM85D9eitO06+UEqivhSMmA08oAdMD7Y5mcyntDttfhdIfmR7ocLE2wHzuyIlkhfL9al2YZPWq1UkJV3KU0Gm3/FlJ0ODoIGadQFxE30tJ/YSxM5NxJdoMWZ5qkdZYkcGXg3Y4MTMmf0uIhYhF4pgQxFKtfPAv+L9M1mKzTrlSB5//mO5j/ZqvZRW8aoyE+hf8RL4WhR4WLmjQk5xdQKzX1Fp+OlilgOg+3AAOjMSV2aOSCthWNLKH3flS2iz07QYTHV3IcjcjALbvGGl0WHMJho4bdf4VabNuL2KDfdQAepej/bu7MS8Byi3lF6dMKjkfP3wuHEjtIRKUn78vX4sHONaZCjzidrrwHRMwJvmEMH/2UiUZnSwcQueWIGAbRRsQfQit4UCAwEAAaNGMEQwDgYDVR0PAQH/BAQDAgeAMBMGA1UdJQQMMAoGCCsGAQUFBwMDMB0GA1UdDgQWBBQiMcsUqvT1pKZXU0koU0HRxkZGJzANBgkqhkiG9w0BAQsFAAOCAYEAHHATeGBka9IAsOLaPKBeiY18dyCYuklelgjrS/ilNDnW+h1avhIPMOhWLFalqKCbtMbGA+wFCuxQ8KnJ9SssfzIAKF6/g2PCg4PjlKhjX1C4eCaHmaoDZoSSO7H5yHtWSCUS/wFbSL0fZJuJukrZq2SSZSz8d/wRuuJTlHMH3xn4SCcFu2VPk0lipSw0O7vuAL1dSMv22pPd0LDKKGWL20MFYJ0nSnjwYOQVTt9LiRA/RE1pyAzWLZGvR/EwhBEikVUYLcvp5hFMeNZ+tPucWsiX8V0Uzcj1/VB7L9UIi+kMEO9iRw54VcYTZQPysYqtN8MMdaVvuhJKb3S2VdgMAmPX4fLt+MPSmzMy5G6zdsYssFibs637jzvV4W4DXfsI7PqZNQt35OEY00s9NLZWW3yE3e08gY/xJVd/MqL/oNhE9nmhrGG33r6nrhkc+YfcsoY4TPdwS43/uqJ32JKFJpu9XXqRyfsTH5RmT4X861sCZoxbe8dfiszWJ/sW24ci'
$RequiredRoles = @('driver_inf', 'signed_catalog', 'authorized_firmware', 'windows_service', 'diagnostic_cli', 'installer', 'installer_engine', 'license')

function Get-CanonicalPath { param([Parameter(Mandatory)][string] $Path) [IO.Path]::GetFullPath($Path) }
function Test-FixedTimeEquals {
    param([Parameter(Mandatory)][byte[]] $Left, [Parameter(Mandatory)][byte[]] $Right)
    if ($Left.Length -ne $Right.Length) { return $false }
    $difference = 0
    for ($index = 0; $index -lt $Left.Length; $index++) { $difference = $difference -bor ($Left[$index] -bxor $Right[$index]) }
    $difference -eq 0
}
function Initialize-DataProtection {
    # Windows PowerShell 5.1 does not load System.Security by default, even
    # though DPAPI is part of the .NET Framework installed with Windows.
    Add-Type -AssemblyName System.Security -ErrorAction Stop
    if (-not ('System.Security.Cryptography.ProtectedData' -as [type])) {
        throw 'The Windows DPAPI assembly (System.Security.Cryptography.ProtectedData) is unavailable.'
    }
}
function Test-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    ([Security.Principal.WindowsPrincipal]::new($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}
function Assert-NoReparsePoint {
    param([Parameter(Mandatory)][string] $Path, [Parameter(Mandatory)][string] $Label)
    $current = Get-CanonicalPath $Path
    while ($true) {
        if (-not (Test-Path -LiteralPath $current)) { throw "$Label is missing: $current" }
        $item = Get-Item -LiteralPath $current -Force
        if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "$Label must not contain a reparse point: $current" }
        $parent = Split-Path -Parent $current
        if ([string]::IsNullOrEmpty($parent) -or $parent -eq $current) { break }
        $current = $parent
    }
}
function Get-EmbeddedCertificate {
    $certificate = [Security.Cryptography.X509Certificates.X509Certificate2]::new([Convert]::FromBase64String($ExpectedCertificateDerBase64))
    if ($certificate.Thumbprint -cne $ExpectedCertificateThumbprint) { throw 'The embedded development signing certificate does not match its pinned thumbprint.' }
    $certificate
}
function Test-ManifestSignature {
    param([Parameter(Mandatory)][string] $ManifestPath)
    Add-Type -AssemblyName System.Security.Cryptography.Pkcs
    $signaturePath = Join-Path (Split-Path -Parent $ManifestPath) 'release-manifest.p7s'
    Assert-NoReparsePoint $ManifestPath 'Release manifest'
    Assert-NoReparsePoint $signaturePath 'Release manifest signature'
    $cms = [Security.Cryptography.Pkcs.SignedCms]::new([Security.Cryptography.Pkcs.ContentInfo]::new([IO.File]::ReadAllBytes($ManifestPath)), $true)
    $cms.Decode([IO.File]::ReadAllBytes($signaturePath))
    $cms.CheckSignature($true)
    if ($cms.SignerInfos.Count -ne 1 -or $cms.SignerInfos[0].Certificate.Thumbprint -cne $ExpectedCertificateThumbprint) {
        throw 'Release manifest was not signed by the pinned PS5 Camera development certificate.'
    }
}
function Test-CatalogSignature {
    param([Parameter(Mandatory)][string] $Catalog, [Parameter()][switch] $UsePlatformTrust)
    if ($UsePlatformTrust) {
        $signature = Get-AuthenticodeSignature -LiteralPath $Catalog
        if ($signature.Status -ne [Management.Automation.SignatureStatus]::Valid -or $signature.SignerCertificate.Thumbprint -cne $ExpectedCertificateThumbprint) {
            throw 'The catalog is not trusted by the explicitly approved development publisher.'
        }
        return
    }
    Add-Type -AssemblyName System.Security.Cryptography.Pkcs
    $cms = [Security.Cryptography.Pkcs.SignedCms]::new()
    $cms.Decode([IO.File]::ReadAllBytes($Catalog))
    $cms.CheckSignature($true)
    if ($cms.SignerInfos.Count -ne 1 -or $cms.SignerInfos[0].Certificate.Thumbprint -cne $ExpectedCertificateThumbprint) {
        throw 'The catalog was not signed by the explicitly approved development publisher.'
    }
}
function Get-VerifiedArtifacts {
    param([Parameter(Mandatory)][string] $ManifestPath)
    $root = Split-Path -Parent (Get-CanonicalPath $ManifestPath)
    Assert-NoReparsePoint $root 'Release root'
    $manifest = Get-Content -LiteralPath $ManifestPath -Raw | ConvertFrom-Json
    if ($manifest.schema_version -ne 1 -or [string]$manifest.release_version -ne $ConfirmReleaseVersion) { throw 'Release version confirmation does not match the signed manifest.' }
    if (@($manifest.hardware_ids).Count -ne 1 -or [string]$manifest.hardware_ids[0] -cne $HardwareId) { throw 'Release manifest has an invalid hardware scope.' }
    $byRole = @{}
    foreach ($artifact in @($manifest.artifacts)) {
        $role = [string]$artifact.role; $name = [string]$artifact.file_name
        if ($role -notin $RequiredRoles -or $byRole.ContainsKey($role) -or $name -ne [IO.Path]::GetFileName($name)) { throw 'Release manifest contains an unsafe artifact declaration.' }
        $path = Get-CanonicalPath (Join-Path $root $name)
        if (-not $path.StartsWith($root.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Artifact escapes the release root.' }
        Assert-NoReparsePoint $path "Release artifact $role"
        if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -cne ([string]$artifact.sha256).ToLowerInvariant()) { throw "Artifact hash mismatch: $role" }
        $byRole[$role] = [ordered]@{ role = $role; name = $name; path = $path; sha256 = ([string]$artifact.sha256).ToLowerInvariant() }
    }
    $actualRoles = (@($byRole.Keys | Sort-Object) -join ',')
    $expectedRoles = (@($RequiredRoles | Sort-Object) -join ',')
    if ($actualRoles -cne $expectedRoles) { throw 'Release manifest must contain exactly the reviewed artifact roles.' }
    if ($byRole.driver_inf.name -cne 'ps5cam-boot.inf' -or $byRole.signed_catalog.name -cne 'ps5cam-boot.cat') { throw 'Release package driver filenames are invalid.' }
    $byRole
}
function Set-PrivateDirectory {
    param([Parameter(Mandatory)][string] $Path, [Parameter()][switch] $AllowUsersRead)
    New-Item -ItemType Directory -Path $Path -Force | Out-Null
    $aclArguments = @($Path, '/inheritance:r', '/grant:r', '*S-1-5-18:(OI)(CI)F', '*S-1-5-32-544:(OI)(CI)F')
    if ($AllowUsersRead) { $aclArguments += '*S-1-5-32-545:(OI)(CI)RX' }
    & icacls.exe @aclArguments *> $null
    if ($LASTEXITCODE -ne 0) { throw "Unable to secure private directory: $Path" }
    Assert-NoReparsePoint $Path 'Private staging directory'
}
function Copy-LockedVerifiedFile {
    param([Parameter(Mandatory)][string] $Source, [Parameter(Mandatory)][string] $Destination, [Parameter(Mandatory)][string] $ExpectedSha256)
    $temp = "$Destination.partial"
    $input = [IO.FileStream]::new($Source, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        $output = [IO.FileStream]::new($temp, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try {
            $hash = [Security.Cryptography.SHA256]::Create(); $buffer = [byte[]]::new(131072)
            while (($read = $input.Read($buffer, 0, $buffer.Length)) -gt 0) { $hash.TransformBlock($buffer, 0, $read, $null, 0) | Out-Null; $output.Write($buffer, 0, $read) }
            $hash.TransformFinalBlock([byte[]]::new(0), 0, 0) | Out-Null
            if (([BitConverter]::ToString($hash.Hash).Replace('-', '')).ToLowerInvariant() -cne $ExpectedSha256) { throw "Artifact changed while being copied: $Source" }
        } finally { $output.Dispose() }
        [IO.File]::Move($temp, $Destination)
    } finally { $input.Dispose(); if (Test-Path -LiteralPath $temp) { Remove-Item -LiteralPath $temp -Force } }
}
function Install-DevelopmentCertificate {
    $certificate = Get-EmbeddedCertificate
    foreach ($storeName in @('Root', 'TrustedPublisher')) {
        $store = [Security.Cryptography.X509Certificates.X509Store]::new($storeName, 'LocalMachine')
        try { $store.Open([Security.Cryptography.X509Certificates.OpenFlags]::ReadWrite); if (@($store.Certificates | Where-Object Thumbprint -ceq $ExpectedCertificateThumbprint).Count -eq 0) { $store.Add($certificate) } }
        finally { $store.Close() }
    }
}
function Invoke-PnpUtil { param([Parameter(Mandatory)][string[]] $Arguments) $output = & pnputil.exe @Arguments 2>&1 | Out-String; if ($LASTEXITCODE -ne 0) { throw "PnPUtil failed: $output" }; $output }
function Get-CameraRuntimeStatus {
    if (-not (Get-Command Get-PnpDevice -ErrorAction SilentlyContinue)) { return 'status_unavailable' }
    $bootObserved = $false
    for ($attempt = 0; $attempt -lt 60; $attempt++) {
        $devices = @(Get-PnpDevice -PresentOnly -ErrorAction SilentlyContinue | Where-Object {
            $_.InstanceId -match '^USB\\VID_05A9&PID_(0580|058C)'
        })
        if (@($devices | Where-Object InstanceId -match '^USB\\VID_05A9&PID_058C').Count -gt 0) { return 'uvc_ready' }
        if (@($devices | Where-Object InstanceId -match '^USB\\VID_05A9&PID_0580').Count -gt 0) { $bootObserved = $true }
        if (-not $bootObserved -and $attempt -ge 8) { return 'camera_not_connected' }
        Start-Sleep -Milliseconds 250
    }
    if ($bootObserved) { return 'boot_detected_uvc_timeout' }
    'camera_not_connected'
}
function Get-StatePaths {
    $stateRoot = Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) $ProductName
    [ordered]@{ root = $stateRoot; state = Join-Path $stateRoot 'development-installer-state.json'; key = Join-Path $stateRoot 'development-installer-state.key' }
}
function Write-AuthenticatedState {
    param([Parameter(Mandatory)][object] $Document, [Parameter(Mandatory)][hashtable] $Paths)
    Initialize-DataProtection
    $key = [byte[]]::new(32); $random = [Security.Cryptography.RandomNumberGenerator]::Create(); try { $random.GetBytes($key) } finally { $random.Dispose() }
    $protected = [System.Security.Cryptography.ProtectedData]::Protect($key, $null, [System.Security.Cryptography.DataProtectionScope]::LocalMachine)
    [IO.File]::WriteAllBytes($Paths.key, $protected)
    $payload = [Text.Encoding]::UTF8.GetBytes(($Document | ConvertTo-Json -Compress -Depth 8))
    $mac = [Security.Cryptography.HMACSHA256]::new($key).ComputeHash($payload)
    $envelope = [ordered]@{ schema_version = 1; payload = [Convert]::ToBase64String($payload); mac = [Convert]::ToBase64String($mac) } | ConvertTo-Json -Compress
    [IO.File]::WriteAllText($Paths.state, $envelope, [Text.UTF8Encoding]::new($false))
}
function Read-AuthenticatedState {
    param([Parameter(Mandatory)][hashtable] $Paths)
    Initialize-DataProtection
    Assert-NoReparsePoint $Paths.root 'Installer state root'; Assert-NoReparsePoint $Paths.key 'Installer state key'; Assert-NoReparsePoint $Paths.state 'Installer state'
    $envelope = Get-Content -LiteralPath $Paths.state -Raw | ConvertFrom-Json
    if ($envelope.schema_version -ne 1) { throw 'Installer state schema is invalid.' }
    $key = [System.Security.Cryptography.ProtectedData]::Unprotect([IO.File]::ReadAllBytes($Paths.key), $null, [System.Security.Cryptography.DataProtectionScope]::LocalMachine)
    $payload = [Convert]::FromBase64String([string]$envelope.payload); $actual = [Security.Cryptography.HMACSHA256]::new($key).ComputeHash($payload)
    if (-not (Test-FixedTimeEquals $actual ([Convert]::FromBase64String([string]$envelope.mac)))) { throw 'Installer state authentication failed.' }
    [Text.Encoding]::UTF8.GetString($payload) | ConvertFrom-Json
}

$paths = Get-StatePaths
$plan = [ordered]@{ action = $Action.ToLowerInvariant(); hardware_id = $HardwareId; release_manifest = $ReleaseManifest; certificate_thumbprint = $ExpectedCertificateThumbprint; requires_administrator = $true; status = 'ready' }
if ($ConfirmDevelopmentCertificateThumbprint -cne $ExpectedCertificateThumbprint) { $plan.status = 'blocked'; $plan.blocker = 'development_certificate_confirmation_required'; $plan.resolution = "Pass -ConfirmDevelopmentCertificateThumbprint $ExpectedCertificateThumbprint to explicitly trust this development publisher." }
if (-not $Execute) {
    if ($plan.status -eq 'ready' -and $Action -ne 'Uninstall') {
        try { if (-not $EmbeddedPayload) { Test-ManifestSignature $ReleaseManifest }; $plan.artifact_roles = @((Get-VerifiedArtifacts $ReleaseManifest).Keys | Sort-Object) }
        catch { $plan.status = 'blocked'; $plan.blocker = 'signed_release_validation_failed'; $plan.resolution = $_.Exception.Message }
    }
    $plan | ConvertTo-Json -Depth 6; exit 0
}
if ($plan.status -ne 'ready') { [Console]::Error.WriteLine(($plan | ConvertTo-Json -Depth 6)); exit 2 }
if (-not (Test-Administrator)) { throw 'Execution requires an elevated Administrator PowerShell session.' }

if ($Action -eq 'Uninstall') {
    $state = Read-AuthenticatedState $paths
    if ($state.hardware_id -cne $HardwareId -or $state.service_name -cne $ServiceName -or [string]$state.published_name -notmatch '^oem\d+\.inf$') { throw 'Installer state does not belong to this PS5 Camera package.' }
    Stop-Service -Name $ServiceName -ErrorAction SilentlyContinue
    & sc.exe delete $ServiceName *> $null
    Remove-Item -LiteralPath "HKLM:\SYSTEM\CurrentControlSet\Services\EventLog\Application\$EventSource" -Recurse -Force -ErrorAction SilentlyContinue
    Invoke-PnpUtil @('/delete-driver', [string]$state.published_name, '/uninstall') | Out-Null
    Remove-Item -LiteralPath ([string]$state.install_root) -Recurse -Force -ErrorAction Stop
    Remove-Item -LiteralPath $paths.state, $paths.key -Force
    if ($RemoveDevelopmentCertificate) {
        foreach ($storeName in @('Root', 'TrustedPublisher')) { $store = [Security.Cryptography.X509Certificates.X509Store]::new($storeName, 'LocalMachine'); try { $store.Open('ReadWrite'); @($store.Certificates | Where-Object Thumbprint -ceq $ExpectedCertificateThumbprint | ForEach-Object { $store.Remove($_) }) } finally { $store.Close() } }
    }
    [ordered]@{ status = 'completed'; action = 'uninstall'; hardware_id = $HardwareId } | ConvertTo-Json -Depth 5; exit 0
}

if ([string]::IsNullOrWhiteSpace($ReleaseManifest) -or [string]::IsNullOrWhiteSpace($ConfirmReleaseVersion)) {
    throw 'Install and Repair require ReleaseManifest and ConfirmReleaseVersion.'
}
if (-not $EmbeddedPayload) { Test-ManifestSignature $ReleaseManifest }
$artifacts = Get-VerifiedArtifacts $ReleaseManifest
$stage = Join-Path $paths.root ('stage-' + [Guid]::NewGuid().ToString('N'))
$installRoot = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) $ProductName
$driverInstalled = $false
try {
    Set-PrivateDirectory $paths.root; Set-PrivateDirectory $stage
    foreach ($artifact in $artifacts.Values) { Copy-LockedVerifiedFile $artifact.path (Join-Path $stage $artifact.name) $artifact.sha256 }
    Install-DevelopmentCertificate
    $catalog = Join-Path $stage $artifacts.signed_catalog.name; $inf = Join-Path $stage $artifacts.driver_inf.name
    Test-CatalogSignature $catalog -UsePlatformTrust:$EmbeddedPayload
    if (Test-Path -LiteralPath $installRoot) {
        if ($Action -ne 'Repair') { throw "Install root already exists: $installRoot. Use Repair or Uninstall." }
        Stop-Service -Name $ServiceName -ErrorAction SilentlyContinue
        & sc.exe delete $ServiceName *> $null
        for ($attempt = 0; $attempt -lt 20 -and (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue); $attempt++) { Start-Sleep -Milliseconds 250 }
        if (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) { throw 'The existing PS5 Camera service did not stop and delete within the repair deadline.' }
        Remove-Item -LiteralPath "HKLM:\SYSTEM\CurrentControlSet\Services\EventLog\Application\$EventSource" -Recurse -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $installRoot -Recurse -Force
    }
    Set-PrivateDirectory $installRoot -AllowUsersRead
    foreach ($artifact in $artifacts.Values | Where-Object role -notin @('driver_inf', 'signed_catalog')) { Copy-LockedVerifiedFile (Join-Path $stage $artifact.name) (Join-Path $installRoot $artifact.name) $artifact.sha256 }
    $output = Invoke-PnpUtil @('/add-driver', $inf, '/install'); $published = @([regex]::Matches($output, '\boem\d+\.inf\b', 'IgnoreCase') | ForEach-Object { $_.Value.ToLowerInvariant() } | Sort-Object -Unique)
    if ($published.Count -ne 1) { throw 'PnPUtil did not report exactly one published driver name.' }; $driverInstalled = $true
    $servicePath = Join-Path $installRoot $artifacts.windows_service.name
    New-Item -Path "HKLM:\SYSTEM\CurrentControlSet\Services\EventLog\Application\$EventSource" -Force | Out-Null
    New-ItemProperty -Path "HKLM:\SYSTEM\CurrentControlSet\Services\EventLog\Application\$EventSource" -Name EventMessageFile -Value $servicePath -PropertyType ExpandString -Force | Out-Null
    New-ItemProperty -Path "HKLM:\SYSTEM\CurrentControlSet\Services\EventLog\Application\$EventSource" -Name TypesSupported -Value 7 -PropertyType DWord -Force | Out-Null
    if (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) { & sc.exe delete $ServiceName *> $null; Start-Sleep -Milliseconds 500 }
    New-Service -Name $ServiceName -BinaryPathName ('"' + $servicePath + '"') -DisplayName 'PS5 Camera Service' -StartupType Automatic | Out-Null
    Start-Service -Name $ServiceName
    Write-AuthenticatedState ([ordered]@{ schema_version = 1; service_name = $ServiceName; hardware_id = $HardwareId; published_name = $published[0]; install_root = $installRoot; release_version = $ConfirmReleaseVersion; stage = $stage }) $paths
    $deviceStatus = Get-CameraRuntimeStatus
    [ordered]@{ status = 'completed'; action = $Action.ToLowerInvariant(); published_name = $published[0]; hardware_id = $HardwareId; device_status = $deviceStatus } | ConvertTo-Json -Compress -Depth 6
}
catch {
    Stop-Service -Name $ServiceName -ErrorAction SilentlyContinue; & sc.exe delete $ServiceName *> $null
    Remove-Item -LiteralPath "HKLM:\SYSTEM\CurrentControlSet\Services\EventLog\Application\$EventSource" -Recurse -Force -ErrorAction SilentlyContinue
    if ($driverInstalled) { try { Invoke-PnpUtil @('/delete-driver', $published[0], '/uninstall') | Out-Null } catch {} }
    Remove-Item -LiteralPath $installRoot -Recurse -Force -ErrorAction SilentlyContinue
    throw
}
