param([Parameter(Mandatory=$true)][string]$Directory)
$ErrorActionPreference='Stop'
$taskRoot=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../temp'))+[IO.Path]::DirectorySeparatorChar
$taskDestination=[IO.Path]::GetFullPath($Directory)
if(-not $taskDestination.StartsWith($taskRoot,[StringComparison]::OrdinalIgnoreCase)){throw 'Certificate fixture must stay under workspace/temp'}
$taskRsa=[Security.Cryptography.RSA]::Create(2048)
try {
    $taskRequest=[Security.Cryptography.X509Certificates.CertificateRequest]::new('CN=atlas-health-loopback',$taskRsa,[Security.Cryptography.HashAlgorithmName]::SHA256,[Security.Cryptography.RSASignaturePadding]::Pkcs1)
    $taskSan=[Security.Cryptography.X509Certificates.SubjectAlternativeNameBuilder]::new()
    $taskSan.AddIpAddress([Net.IPAddress]::Parse('127.0.0.1'))
    $taskRequest.CertificateExtensions.Add($taskSan.Build())
    $taskCert=$taskRequest.CreateSelfSigned([DateTimeOffset]::UtcNow.AddMinutes(-5),[DateTimeOffset]::UtcNow.AddHours(1))
    try {
        [IO.File]::WriteAllText((Join-Path $taskDestination 'cert.pem'),$taskCert.ExportCertificatePem())
        [IO.File]::WriteAllText((Join-Path $taskDestination 'key.pem'),$taskRsa.ExportPkcs8PrivateKeyPem())
    } finally {$taskCert.Dispose()}
} finally {$taskRsa.Dispose()}
