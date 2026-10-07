param([Parameter(Mandatory)][string]$SourceFile, [switch]$Redirect, [switch]$Gzip)
$ErrorActionPreference = 'Stop'
# Real loopback HTTPS publisher for acceptance tests. Certificate trust is scoped
# to the Rust test client; nothing is installed in the Windows certificate store.
$rsa = [System.Security.Cryptography.RSA]::Create(2048)
$request = [System.Security.Cryptography.X509Certificates.CertificateRequest]::new('CN=localhost', $rsa, [System.Security.Cryptography.HashAlgorithmName]::SHA256, [System.Security.Cryptography.RSASignaturePadding]::Pkcs1)
$san = [System.Security.Cryptography.X509Certificates.SubjectAlternativeNameBuilder]::new()
$san.AddDnsName('localhost')
$san.AddIpAddress([System.Net.IPAddress]::Loopback)
$request.CertificateExtensions.Add($san.Build())
$certificate = $request.CreateSelfSigned([DateTimeOffset]::UtcNow.AddMinutes(-5), [DateTimeOffset]::UtcNow.AddHours(1))
$exported = $certificate.Export([System.Security.Cryptography.X509Certificates.X509ContentType]::Pfx, 'atlas-test')
$certificate.Dispose()
$certificate = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($exported, 'atlas-test')
$listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
$listener.Start()
$body = [System.Text.Encoding]::UTF8.GetBytes((Get-Content -LiteralPath $SourceFile -Raw).Trim())
$encoded = [System.Text.Encoding]::UTF8.GetBytes([Convert]::ToBase64String($body))
$encodingHeader=''
if ($Gzip) {
    $buffer=[System.IO.MemoryStream]::new()
    $compressor=[System.IO.Compression.GZipStream]::new($buffer,[System.IO.Compression.CompressionMode]::Compress,$true)
    $compressor.Write($encoded);$compressor.Dispose()
    $encoded=$buffer.ToArray();$buffer.Dispose()
    $encodingHeader="Content-Encoding: gzip`r`n"
}
@{ port=$listener.LocalEndpoint.Port; certificate=[Convert]::ToBase64String($certificate.Export([System.Security.Cryptography.X509Certificates.X509ContentType]::Cert)) } | ConvertTo-Json -Compress
try {
    while ($true) {
        $client=$listener.AcceptTcpClient()
        $tls=[System.Net.Security.SslStream]::new($client.GetStream(), $false)
        try {
            $tls.ReadTimeout=5000
            $tls.AuthenticateAsServer($certificate, $false, [System.Security.Authentication.SslProtocols]::Tls12, $false)
            $reader=[System.IO.StreamReader]::new($tls, [System.Text.Encoding]::ASCII, $false, 1024, $true)
            $requestLine=$reader.ReadLine()
            while ($reader.ReadLine()) {}
            $redirectLocation = switch -Regex ($requestLine) {
                '^GET /subscription ' { if ($Redirect) {'/body'} }
                '^GET /unsafe ' {'http://127.0.0.1/body'}
                '^GET /cross-origin ' {'https://different-origin.invalid/body'}
                '^GET /loop ' {'/loop'}
            }
            if ($redirectLocation) {
                $header=[System.Text.Encoding]::ASCII.GetBytes("HTTP/1.1 301 Moved Permanently`r`nLocation: $redirectLocation`r`nContent-Length: 0`r`nConnection: close`r`n`r`n")
                $tls.Write($header); $tls.Flush()
                continue
            }
            $header=[System.Text.Encoding]::ASCII.GetBytes("HTTP/1.1 200 OK`r`nContent-Type: text/plain`r`n${encodingHeader}Content-Length: $($encoded.Length)`r`nConnection: close`r`n`r`n")
            $tls.Write($header); $tls.Write($encoded); $tls.Flush()
        } finally { $tls.Dispose(); $client.Dispose() }
    }
} finally { $listener.Stop(); $certificate.Dispose(); $rsa.Dispose() }
