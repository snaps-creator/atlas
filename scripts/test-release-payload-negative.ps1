param([Parameter(Mandatory=$true)][string]$Payload,[Parameter(Mandatory=$true)][string]$Installer,[string]$OldComponent)
$ErrorActionPreference='Stop'
$contract=Join-Path $PSScriptRoot 'test-release-payload.ps1'
$expectedVersion=(Get-Content (Join-Path (Split-Path $PSScriptRoot -Parent) 'src-tauri/tauri.conf.json') -Raw|ConvertFrom-Json).version
if ($expectedVersion -notmatch '^\d+\.\d+\.\d+$') {throw 'Negative version validation requires a stable product version'}
$lastDigit=[int]::Parse($expectedVersion.Substring($expectedVersion.Length-1))
$wrongVersion=$expectedVersion.Substring(0,$expectedVersion.Length-1)+[string](($lastDigit+1)%10)
$fixture=Join-Path $env:TEMP ('atlas-contract-negative-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture|Out-Null
Get-ChildItem -LiteralPath $Payload|Copy-Item -Destination $fixture -Recurse
$results=@()
try {
    & $contract -Payload $fixture -Installer $Installer
    $results+=@{case='valid transactional payload';result='PASS'}
    foreach($name in @('AtlasUpdater.exe','Atlas.Service.exe','Atlas.exe','libcef.dll')) {
        $path=Join-Path $fixture $name;$saved="$path.negative-save"
        Move-Item -LiteralPath $path -Destination $saved
        try {
            $rejected=$false
            try { & $contract -Payload $fixture -Installer $Installer } catch {
                if ($_.Exception.Message -notlike "*required file missing: $name*") {throw}
                $rejected=$true
            }
            if(-not $rejected){throw "Missing $name was accepted"}
            $results+=@{case="missing $name";result='PASS (rejected)'}
        } finally {Move-Item -LiteralPath $saved -Destination $path}
    }
    $name='AtlasMaintenance.exe';$path=Join-Path $fixture $name;$saved="$path.negative-save"
    Move-Item -LiteralPath $path -Destination $saved
    try {
        if ($OldComponent) {
            if((Get-Item -LiteralPath $OldComponent).VersionInfo.ProductVersion -eq $expectedVersion){throw 'Negative version fixture must actually be from an older release'}
            Copy-Item -LiteralPath $OldComponent -Destination $path
        } else {
            # Change version-resource text only in this disposable executable.
            # It is never executed; this verifies metadata rejection before the
            # receipt/hash rejection would catch the intentionally altered bytes.
            & node -e "const fs=require('fs');const b=fs.readFileSync(process.argv[1]),a=Buffer.from(process.argv[3],'utf16le'),z=Buffer.from(process.argv[4],'utf16le');let n=0;for(let i=b.indexOf(a);i>=0;i=b.indexOf(a,i+a.length)){z.copy(b,i);n++}if(!n)throw Error('Version fixture not found');fs.writeFileSync(process.argv[2],b)" $saved $path $expectedVersion $wrongVersion
            if($LASTEXITCODE -ne 0 -or (Get-Item -LiteralPath $path).VersionInfo.ProductVersion -ne $wrongVersion){throw 'Negative fixture did not change actual PE product metadata'}
        }
        $rejected=$false
        try { & $contract -Payload $fixture -Installer $Installer } catch {
            if($_.Exception.Message -notlike '*wrong product version: AtlasMaintenance.exe*'){throw}
            $rejected=$true
        }
        if(-not $rejected){throw 'Old component version was accepted'}
        $results+=@{case='AtlasMaintenance with actual mismatched PE product version';result='PASS (rejected)'}
    } finally {Remove-Item -LiteralPath $path -ErrorAction SilentlyContinue;Move-Item -LiteralPath $saved -Destination $path}
    & $contract -Payload $fixture -Installer $Installer
    $results+=@{case='restored payload';result='PASS'}
    $results|ConvertTo-Json
} finally {
    $resolved=[IO.Path]::GetFullPath($fixture);$tempRoot=[IO.Path]::GetFullPath($env:TEMP).TrimEnd('\')+'\'
    if(-not $resolved.StartsWith($tempRoot,[StringComparison]::OrdinalIgnoreCase) -or (Split-Path $resolved -Leaf) -notlike 'atlas-contract-negative-*'){throw 'Unsafe negative fixture cleanup'}
    Remove-Item -LiteralPath $resolved -Recurse -Force
}