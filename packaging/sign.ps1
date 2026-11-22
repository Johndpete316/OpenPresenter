# Sign files with Azure Artifact Signing (signtool + the Artifact Signing dlib).
#
#   packaging/sign.ps1 <file> [<file> ...]
#
# Used by .github/workflows/release.yml for the program files, and by Inno Setup
# (SignTool) for the installer and its uninstaller. Authentication is whatever
# DefaultAzureCredential finds; in CI that's the `az` login from azure/login (OIDC).
#
# Env (all required):
#   OP_SIGN_DLIB      path to Azure.CodeSigning.Dlib.dll (bin\x64 of the
#                     Microsoft.ArtifactSigning.Client NuGet package)
#   OP_SIGN_ENDPOINT  e.g. https://eus.codesigning.azure.net/
#   OP_SIGN_ACCOUNT   Artifact Signing account name
#   OP_SIGN_PROFILE   certificate profile name
$ErrorActionPreference = 'Stop'

if ($args.Count -eq 0) { throw 'sign.ps1: no files given' }
foreach ($v in 'OP_SIGN_DLIB', 'OP_SIGN_ENDPOINT', 'OP_SIGN_ACCOUNT', 'OP_SIGN_PROFILE') {
    if (-not [Environment]::GetEnvironmentVariable($v)) { throw "sign.ps1: $v isn't set" }
}

# Newest x64 signtool from the Windows SDK.
$signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" |
    Sort-Object { [version]($_.Directory.Parent.Name) } -Descending |
    Select-Object -First 1
if (-not $signtool) { throw 'sign.ps1: signtool.exe not found (install the Windows SDK)' }

$metadata = Join-Path ([IO.Path]::GetTempPath()) 'openpresenter-signing.json'
@{
    Endpoint               = $env:OP_SIGN_ENDPOINT
    CodeSigningAccountName = $env:OP_SIGN_ACCOUNT
    CertificateProfileName = $env:OP_SIGN_PROFILE
    ExcludeCredentials     = @('ManagedIdentityCredential', 'WorkloadIdentityCredential',
        'SharedTokenCacheCredential', 'VisualStudioCredential', 'VisualStudioCodeCredential',
        'AzurePowerShellCredential', 'AzureDeveloperCliCredential', 'InteractiveBrowserCredential')
} | ConvertTo-Json | Set-Content -Encoding UTF8 $metadata

# The timestamp keeps signatures valid after the short-lived certificate expires.
& $signtool.FullName sign /v /fd SHA256 /tr 'http://timestamp.acs.microsoft.com' /td SHA256 `
    /dlib $env:OP_SIGN_DLIB /dmdf $metadata @args
if ($LASTEXITCODE -ne 0) { throw "signtool failed ($LASTEXITCODE)" }

foreach ($f in $args) {
    $s = Get-AuthenticodeSignature -LiteralPath $f
    if ($s.Status -ne 'Valid') { throw "sign.ps1: $f isn't validly signed ($($s.Status): $($s.StatusMessage))" }
    Write-Host "signed: $f ($($s.SignerCertificate.Subject))"
}
