#requires -Version 7.0

[CmdletBinding(SupportsShouldProcess, ConfirmImpact = 'High')]
param(
    [Parameter()]
    [ValidatePattern('^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$')]
    [string] $Repository = 'valtoni/PS5-Camera',

    [Parameter()]
    [ValidatePattern('^[0-9A-Fa-f]{40}$')]
    [string] $CertificateThumbprint = 'EDAF55A1E4AE0C8C197988F7286626BD51228CA2',

    [Parameter()]
    [Security.SecureString] $PfxPassword,

    [Parameter()]
    [ValidatePattern('^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$')]
    [string] $DispatchReleaseVersion
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$pfxSecretName = 'PS5CAM_SIGNING_PFX_BASE64'
$passwordSecretName = 'PS5CAM_SIGNING_PFX_PASSWORD'
$releaseTokenSecretName = 'PS5CAM_RELEASE_TOKEN'
$normalizedThumbprint = $CertificateThumbprint.Replace(' ', '').ToUpperInvariant()

function New-RandomPfxPassword {
    $bytes = [byte[]]::new(32)
    [Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
    return (ConvertTo-SecureString -String ([Convert]::ToHexString($bytes)) -AsPlainText -Force)
}

function Set-GitHubActionsSecret {
    param(
        [Parameter(Mandatory)][string] $GitHubCli,
        [Parameter(Mandatory)][string] $SecretName,
        [Parameter(Mandatory)][string] $Value
    )

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $GitHubCli
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardInput = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in @('secret', 'set', '--repo', $Repository, $SecretName)) {
        $null = $startInfo.ArgumentList.Add($argument)
    }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) { throw "Unable to start GitHub CLI while setting $SecretName." }
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    $process.StandardInput.Write($Value)
    $process.StandardInput.Close()
    $process.WaitForExit()
    $null = $stdout.GetAwaiter().GetResult()
    $null = $stderr.GetAwaiter().GetResult()
    if ($process.ExitCode -ne 0) { throw "Unable to set $SecretName." }
}

$githubCliCommand = Get-Command gh -ErrorAction SilentlyContinue
if ($null -eq $githubCliCommand) {
    throw 'GitHub CLI (gh) is required. Install it, run "gh auth login", then retry.'
}
$githubCli = $githubCliCommand.Source

$certificatePath = "Cert:\LocalMachine\My\$normalizedThumbprint"
if (-not (Test-Path -LiteralPath $certificatePath)) {
    throw "The development signing certificate was not found: $certificatePath"
}

$certificate = Get-Item -LiteralPath $certificatePath
if (-not $certificate.HasPrivateKey) {
    throw 'The development signing certificate has no accessible private key.'
}

if (-not $PSCmdlet.ShouldProcess($Repository, "replace $pfxSecretName, $passwordSecretName, and $releaseTokenSecretName Actions Secrets")) {
    return
}

if ($null -eq $PfxPassword) {
    $PfxPassword = New-RandomPfxPassword
}

$temporaryPfx = Join-Path ([IO.Path]::GetTempPath()) ('ps5cam-release-signer-' + [Guid]::NewGuid().ToString('N') + '.pfx')
$passwordPointer = [IntPtr]::Zero
try {
    Export-PfxCertificate -Cert $certificate.PSPath -FilePath $temporaryPfx -Password $PfxPassword -ChainOption EndEntityCertOnly | Out-Null
    $encodedPfx = [Convert]::ToBase64String([IO.File]::ReadAllBytes($temporaryPfx))
    if ($encodedPfx.Length -gt (48KB)) {
        throw 'The encoded PFX exceeds GitHub Actions'' 48 KB secret limit. Use a smaller end-entity PFX.'
    }
    $passwordPointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($PfxPassword)
    $plainPassword = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($passwordPointer)
    $releaseToken = (& $githubCli auth token 2>$null | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($releaseToken)) {
        throw 'GitHub CLI authentication must expose a repository token before release publishing can be configured.'
    }

    Set-GitHubActionsSecret -GitHubCli $githubCli -SecretName $passwordSecretName -Value $plainPassword
    Set-GitHubActionsSecret -GitHubCli $githubCli -SecretName $pfxSecretName -Value $encodedPfx
    Set-GitHubActionsSecret -GitHubCli $githubCli -SecretName $releaseTokenSecretName -Value $releaseToken

    $workflowDispatched = $false
    if (-not [string]::IsNullOrWhiteSpace($DispatchReleaseVersion)) {
        & $githubCli workflow run 'Build and publish Windows setup' --repo $Repository --ref master -f "release_version=$DispatchReleaseVersion"
        if ($LASTEXITCODE -ne 0) { throw "Unable to dispatch release version $DispatchReleaseVersion." }
        $workflowDispatched = $true
    }

    [pscustomobject]@{
        repository = $Repository
        certificate_thumbprint = $normalizedThumbprint
        configured_secrets = @($pfxSecretName, $passwordSecretName, $releaseTokenSecretName)
        workflow_dispatched = $workflowDispatched
        dispatched_release_version = $DispatchReleaseVersion
    } | ConvertTo-Json -Depth 3
}
finally {
    $plainPassword = $null
    $encodedPfx = $null
    $releaseToken = $null
    if ($passwordPointer -ne [IntPtr]::Zero) {
        [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($passwordPointer)
    }
    Remove-Item -LiteralPath $temporaryPfx -Force -ErrorAction SilentlyContinue
}
