$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
function Section($name, [scriptblock]$read) {
    Write-Output "`n=== $name ==="
    try { & $read | Format-List | Out-String -Width 240 | Write-Output }
    catch { Write-Output ("Unavailable: " + $_.Exception.Message) }
}
Section 'Collector identity' {
    [pscustomobject]@{Time=(Get-Date -Format o);Elevated=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)}
}
$filterIds = @()
Section 'WFP event collection status' { & "$env:SystemRoot\System32\netsh.exe" wfp show options optionsfor=netevents }
Section 'Installed service and core binary identity' {
    Get-CimInstance Win32_Process | Where-Object {$_.Name -in @('atlas-vpn.exe','mihomo.exe','verge-mihomo.exe','verge-mihomo-alpha.exe')} |
        ForEach-Object {
            $process = $_
            if ($process.ExecutablePath) {
                $binary = Get-Item -LiteralPath $process.ExecutablePath
                [pscustomobject]@{Name=$process.Name;PID=$process.ProcessId;Parent=$process.ParentProcessId;Path=$binary.FullName;
                    Version=$binary.VersionInfo.FileVersion;SHA256=(Get-FileHash -LiteralPath $binary.FullName -Algorithm SHA256).Hash}
            }
        }
}
Section 'Recent WFP events for running cores' {
    Get-CimInstance Win32_Process | Where-Object {$_.Name -in @('mihomo.exe','verge-mihomo.exe','verge-mihomo-alpha.exe')} |
        Select-Object -First 3 | ForEach-Object {
            if ($_.ExecutablePath) {
                $eventText = (& "$env:SystemRoot\System32\netsh.exe" wfp show netevents file=- "appid=$($_.ExecutablePath)" timewindow=1800 | Out-String)
                Write-Output $eventText
                foreach ($match in [regex]::Matches($eventText,'(?i)<filterId>\s*(\d+)\s*</filterId>')) { $script:filterIds += $match.Groups[1].Value }
            }
        }
}
Section 'Recorded Windows filtering drops (absence does not prove no drops)' {
    $events = Get-WinEvent -FilterHashtable @{LogName='Security';Id=5152,5157;StartTime=(Get-Date).AddMinutes(-30)} -MaxEvents 150
    foreach ($event in $events) {
        [xml]$xml = $event.ToXml()
        $data = @{}
        foreach ($item in $xml.Event.EventData.Data) { $data[$item.Name] = $item.'#text' }
        if ($data.Application -match '(?i)atlas|mihomo|clash') {
            $script:filterIds += $data.FilterRTID
            [pscustomobject]@{Time=$event.TimeCreated;Event=$event.Id;Application=$data.Application;ProcessId=$data.ProcessID;
                Source=$data.SourceAddress;SourcePort=$data.SourcePort;Destination=$data.DestAddress;DestinationPort=$data.DestPort;
                Protocol=$data.Protocol;Direction=$data.Direction;FilterId=$data.FilterRTID;LayerId=$data.LayerRTID}
        }
    }
}
Section 'WFP filters: Atlas, Mihomo and recorded drop IDs' {
    # The service creates this directory in its own protected temporary location.
    # No caller-controlled path or executable is accepted.
    $directory = Join-Path ([IO.Path]::GetTempPath()) ('atlas-wfp-report-' + [guid]::NewGuid().ToString('N'))
    [IO.Directory]::CreateDirectory($directory) | Out-Null
    $file = Join-Path $directory 'state.xml'
    try {
        & "$env:SystemRoot\System32\netsh.exe" wfp show state "file=$file" | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "netsh WFP exit $LASTEXITCODE" }
        if ((Get-Item -LiteralPath $file).Length -gt 64MB) { throw 'WFP state exceeds 64 MiB parsing limit' }
        $reader = [Xml.XmlReaderSettings]::new()
        $reader.DtdProcessing = [Xml.DtdProcessing]::Prohibit
        $reader.XmlResolver = $null
        # Some Windows builds append multiple XML documents to show-state output.
        # Keep every document under a synthetic root; DTDs remain prohibited.
        $raw = [IO.File]::ReadAllText($file)
        $raw = [regex]::Replace($raw,'<\?xml[^?]*\?>','')
        $textReader = [IO.StringReader]::new('<atlasWfp>' + $raw + '</atlasWfp>')
        $stream = [Xml.XmlReader]::Create($textReader,$reader)
        try { $doc = [Xml.XmlDocument]::new(); $doc.XmlResolver=$null; $doc.Load($stream) } finally { $stream.Dispose() }
        $items = @($doc.SelectNodes('//*[local-name()="filters"]/*[local-name()="item"]'))
        Write-Output ("Total filters: " + $items.Count)
        $matched = @($items | Where-Object { $_.displayData.name -match '(?i)atlas|атлас|mihomo|clash|sing-tun' -or $_.filterId -in $script:filterIds })
        Write-Output ("Matching filters: " + $matched.Count)
        foreach ($item in ($matched | Select-Object -First 200)) { Write-Output $item.OuterXml }
        if ($matched.Count -gt 200) { Write-Output '[Filter details truncated to 200]' }
    } finally {
        # Delete only the two exact objects created here; never recurse.
        if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file -Force }
        if (Test-Path -LiteralPath $directory) { Remove-Item -LiteralPath $directory -Force }
    }
}
Section 'Windows firewall profiles' { Get-NetFirewallProfile | Select-Object Name,Enabled,DefaultInboundAction,DefaultOutboundAction,LogBlocked,LogAllowed }
Section 'Relevant system events: interface, TCP, DNS, sleep, service crash' {
    Get-WinEvent -FilterHashtable @{LogName='System';StartTime=(Get-Date).AddHours(-2)} -MaxEvents 300 |
        Where-Object {$_.ProviderName -match 'Tcpip|NDIS|DNS.Client|Dhcp|Kernel.Power|Power.Troubleshooter|Service.Control.Manager|Schannel'} |
        Select-Object -First 70 TimeCreated,ProviderName,Id,LevelDisplayName,Message
}
