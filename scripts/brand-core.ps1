param([string]$Executable = (Join-Path (Split-Path $PSScriptRoot -Parent) 'src-tauri\resources\Atlas.Core.exe'), [ValidateSet('Mihomo','Xray')][string]$Engine = 'Mihomo')
$ErrorActionPreference = 'Stop'
if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) { throw "Core not found: $Executable" }
if ((Get-AuthenticodeSignature -LiteralPath $Executable).Status -ne 'NotSigned') {
    throw 'Refusing to change a signed third-party executable'
}

if (-not ('AtlasVersionResource' -as [type])) { Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class AtlasVersionResource {
    [DllImport("kernel32.dll", EntryPoint="BeginUpdateResourceW", CharSet=CharSet.Unicode, SetLastError=true)]
    public static extern IntPtr Begin(string path, bool deleteExisting);
    [DllImport("kernel32.dll", EntryPoint="UpdateResourceW", CharSet=CharSet.Unicode, SetLastError=true)]
    public static extern bool Update(IntPtr handle, IntPtr type, IntPtr name, ushort language, byte[] data, uint length);
    [DllImport("kernel32.dll", EntryPoint="EndUpdateResourceW", CharSet=CharSet.Unicode, SetLastError=true)]
    public static extern bool Finish(IntPtr handle, bool discard);
}
'@ }

function New-VersionBlock([string]$Key, [byte[]]$Value, [int]$ValueLength, [int]$Type, [byte[][]]$Children) {
    $stream = [IO.MemoryStream]::new()
    $writer = [IO.BinaryWriter]::new($stream)
    $writer.Write([uint16]0)
    $writer.Write([uint16]$ValueLength)
    $writer.Write([uint16]$Type)
    $writer.Write([Text.Encoding]::Unicode.GetBytes($Key + [char]0))
    while ($stream.Position % 4) { $writer.Write([byte]0) }
    $writer.Write($Value)
    while ($stream.Position % 4) { $writer.Write([byte]0) }
    foreach ($child in $Children) { $writer.Write([byte[]]$child) }
    $bytes = $stream.ToArray()
    if ($bytes.Length -gt [uint16]::MaxValue) { throw 'Version resource too large' }
    [BitConverter]::GetBytes([uint16]$bytes.Length).CopyTo($bytes, 0)
    $writer.Dispose()
    $stream.Dispose()
    return ,$bytes
}
function New-VersionString([string]$Key, [string]$Value) {
    $bytes = [Text.Encoding]::Unicode.GetBytes($Value + [char]0)
    return New-VersionBlock $Key $bytes ($Value.Length + 1) 1 @()
}

$component = if ($Engine -eq 'Xray') { 'Atlas.Xray' } else { 'Atlas.Core' }
$engineVersion = if ($Engine -eq 'Xray') { [Version]'26.3.27.0' } else { [Version]'1.19.29.0' }
$strings = @(
    (New-VersionString 'FileDescription' "$component — $Engine network engine"),
    (New-VersionString 'ProductName' "Atlas Core ($Engine)"),
    (New-VersionString 'FileVersion' $engineVersion.ToString()),
    (New-VersionString 'OriginalFilename' "$component.exe"),
    (New-VersionString 'Comments' "$Engine open-source component; see LICENSE-$($Engine.ToLowerInvariant())")
)
$table = New-VersionBlock '040904B0' ([byte[]]@()) 0 1 $strings
$stringFileInfo = New-VersionBlock 'StringFileInfo' ([byte[]]@()) 0 1 @($table)
$translation = New-VersionBlock 'Translation' ([byte[]](0x09,0x04,0xB0,0x04)) 4 0 @()
$varFileInfo = New-VersionBlock 'VarFileInfo' ([byte[]]@()) 0 1 @($translation)
$fixedStream = [IO.MemoryStream]::new()
$fixedWriter = [IO.BinaryWriter]::new($fixedStream)
foreach ($value in @(
    4277077181, 0x00010000, (($engineVersion.Major -shl 16) -bor $engineVersion.Minor), (($engineVersion.Build -shl 16) -bor $engineVersion.Revision),
    (($engineVersion.Major -shl 16) -bor $engineVersion.Minor), (($engineVersion.Build -shl 16) -bor $engineVersion.Revision), 0x0000003F, 0,
    0x00040004, 1, 0, 0, 0
)) { $fixedWriter.Write([uint32]$value) }
$version = New-VersionBlock 'VS_VERSION_INFO' $fixedStream.ToArray() 52 0 @($stringFileInfo, $varFileInfo)
$fixedWriter.Dispose()
$fixedStream.Dispose()

$handle = [AtlasVersionResource]::Begin($Executable, $false)
if ($handle -eq [IntPtr]::Zero) { throw "BeginUpdateResource failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
$updated = [AtlasVersionResource]::Update($handle, [IntPtr]16, [IntPtr]1, [uint16]0x0409, $version, [uint32]$version.Length)
if (-not $updated) {
    $errorCode = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
    $discarded = [AtlasVersionResource]::Finish($handle, $true)
    throw "UpdateResource failed: $errorCode"
}
$committed = [AtlasVersionResource]::Finish($handle, $false)
if (-not $committed) {
    throw "EndUpdateResource failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
}
$info = (Get-Item -LiteralPath $Executable).VersionInfo
if (-not $info.FileDescription.StartsWith('Atlas') -or $info.FileVersion -ne $engineVersion.ToString()) {
    throw 'Version resource verification failed'
}
Write-Output "$($info.FileDescription) $($info.FileVersion)"
