param([switch]$Force)

$ErrorActionPreference = 'Stop'
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path

# Remove legacy AgenticTerminal install (product was renamed to IntelligentTerminal)
$legacyAppx = Get-AppxPackage *AgenticTerminal* -ErrorAction SilentlyContinue
if ($legacyAppx) {
    Write-Host "Removing legacy AgenticTerminal package ($($legacyAppx.Version))..."
    $legacyAppx | Remove-AppxPackage
}

$legacyUnpackagedPath = "$env:LOCALAPPDATA\Programs\AgenticTerminal"
if (Test-Path $legacyUnpackagedPath) {
    Write-Host "Removing legacy unpackaged install at $legacyUnpackagedPath..."
    Remove-Item $legacyUnpackagedPath -Recurse -Force
}

$legacyStartMenu = "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\Agentic Terminal"
if (Test-Path $legacyStartMenu) {
    Write-Host "Removing legacy Start menu folder $legacyStartMenu..."
    Remove-Item $legacyStartMenu -Recurse -Force
}

# Remove any existing IntelligentTerminal registration (packaged or unpackaged)
$existing = Get-AppxPackage *IntelligentTerminal* -ErrorAction SilentlyContinue
if ($existing) {
    Write-Host "Removing existing IntelligentTerminal package ($($existing.Version))..."
    $existing | Remove-AppxPackage
}

# Remove old unpackaged install files if present
$unpackagedPath = "$env:LOCALAPPDATA\Programs\IntelligentTerminal"
if (Test-Path $unpackagedPath) {
    Write-Host "Removing old unpackaged install at $unpackagedPath..."
    Remove-Item $unpackagedPath -Recurse -Force
}

# Trust the dev certificate only if not already trusted
$cer = Get-Item "$scriptDir\IntelligentTerminalDev.cer" -ErrorAction Stop
$cerBytes = [System.IO.File]::ReadAllBytes($cer.FullName)
$x509 = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($cerBytes)
$store = [System.Security.Cryptography.X509Certificates.X509Store]::new('TrustedPeople', 'LocalMachine')
$store.Open('ReadOnly')
$alreadyTrusted = $store.Certificates | Where-Object { $_.Thumbprint -eq $x509.Thumbprint }
$store.Close()

if (-not $alreadyTrusted) {
    Write-Host "Importing dev certificate (requires admin)..."
    Import-Certificate -FilePath $cer.FullName -CertStoreLocation Cert:\LocalMachine\TrustedPeople | Out-Null
} else {
    Write-Host "Dev certificate already trusted, skipping import."
}

# Install XAML dependency
$xaml = Get-Item "$scriptDir\Dependencies\Microsoft.UI.Xaml.2.8.appx" -ErrorAction SilentlyContinue
if ($xaml) {
    Write-Host "Installing XAML dependency..."
    Add-AppxPackage -Path $xaml.FullName -ErrorAction SilentlyContinue
}

# Install the MSIX
$msix = Get-Item "$scriptDir\CascadiaPackage_*.msix" -ErrorAction Stop | Select-Object -First 1
Write-Host "Installing $($msix.Name)..."
Add-AppxPackage -Path $msix.FullName

Write-Host "Done. Launch 'Intelligent Terminal (Dev)' from the Start menu."
