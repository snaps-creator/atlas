# Windows native utilities emit OEM bytes into redirected pipes. Decode them
# before returning to the UTF-8 stream consumed by the Rust report collector.
function Invoke-AtlasNative([scriptblock]$read) {
    if (-not ('AtlasReportCodePage' -as [type])) {
        Add-Type -TypeDefinition 'public static class AtlasReportCodePage { [System.Runtime.InteropServices.DllImport("kernel32.dll")] public static extern uint GetOEMCP(); }'
    }
    $previous = [Console]::OutputEncoding
    try {
        [Console]::OutputEncoding = [Text.Encoding]::GetEncoding([int][AtlasReportCodePage]::GetOEMCP())
        $captured = & $read | Out-String -Width 240
    } finally {
        [Console]::OutputEncoding = $previous
    }
    $captured
}
