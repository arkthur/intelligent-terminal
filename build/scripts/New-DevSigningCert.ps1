param(
    [string]$PfxPath = 'cert\IntelligentTerminalDev.pfx',
    [string]$CerPath = 'artifacts\local-installer\IntelligentTerminalDev.cer',
    [string]$Subject = 'CN=Intelligent Terminal Dev'
)

$ErrorActionPreference = 'Stop'

New-Item -ItemType Directory -Path (Split-Path $PfxPath -Parent) -Force | Out-Null
New-Item -ItemType Directory -Path (Split-Path $CerPath  -Parent) -Force | Out-Null

# Use pure .NET so this works in both Windows PowerShell and pwsh
# without needing the Cert: PSDrive or PKI module.
Add-Type -AssemblyName System.Security

$rsa = [System.Security.Cryptography.RSA]::Create(2048)

$req = [System.Security.Cryptography.X509Certificates.CertificateRequest]::new(
    $Subject,
    $rsa,
    [System.Security.Cryptography.HashAlgorithmName]::SHA256,
    [System.Security.Cryptography.RSASignaturePadding]::Pkcs1
)

# Code-signing EKU
$ekuOids = [System.Security.Cryptography.OidCollection]::new()
$ekuOids.Add([System.Security.Cryptography.Oid]::new('1.3.6.1.5.5.7.3.3')) | Out-Null
$req.CertificateExtensions.Add(
    [System.Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]::new($ekuOids, $false))

# Basic constraints (not a CA)
$req.CertificateExtensions.Add(
    [System.Security.Cryptography.X509Certificates.X509BasicConstraintsExtension]::new($false, $false, 0, $false))

$notBefore = [DateTimeOffset]::UtcNow.AddDays(-1)
$notAfter  = [DateTimeOffset]::UtcNow.AddYears(3)
$cert = $req.CreateSelfSigned($notBefore, $notAfter)

Write-Host "Subject:  $($cert.Subject)"
Write-Host "NotAfter: $($cert.NotAfter)"

# Export PFX (empty password)
$pfxBytes = $cert.Export([System.Security.Cryptography.X509Certificates.X509ContentType]::Pfx)
[System.IO.File]::WriteAllBytes((Resolve-Path '.').Path + '\' + $PfxPath, $pfxBytes)

# Export CER (public key only)
$cerBytes = $cert.Export([System.Security.Cryptography.X509Certificates.X509ContentType]::Cert)
[System.IO.File]::WriteAllBytes((Resolve-Path '.').Path + '\' + $CerPath, $cerBytes)

Write-Host "PFX: $PfxPath ($((Get-Item $PfxPath).Length) bytes)"
Write-Host "CER: $CerPath ($((Get-Item $CerPath).Length) bytes)"
