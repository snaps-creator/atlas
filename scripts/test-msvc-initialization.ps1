$ErrorActionPreference='Stop'
$originalPath=$env:PATH;$originalExport=$env:GITHUB_ENV
$fixture=Join-Path $env:TEMP ('atlas-msvc-acceptance-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture|Out-Null
try {
    $env:GITHUB_ENV=Join-Path $fixture 'github-env.txt'
    . (Join-Path $PSScriptRoot 'initialize-msvc.ps1')
    $baseline=$env:PATH
    1..3|ForEach-Object {
        . (Join-Path $PSScriptRoot 'initialize-msvc.ps1')
        if($env:PATH-ne $baseline){throw 'Repeated MSVC initialization changed PATH'}
    }
    $exports=Get-Content -LiteralPath $env:GITHUB_ENV -Raw
    foreach($name in @('VSCMD_VER','VSCMD_ARG_TGT_ARCH')) {if($exports-notmatch "(?m)^$name="){throw "CI does not preserve $name across steps"}}
    # cmd expansion fails above 8191 characters. An already initialized native
    # compiler must remain usable without re-entering VsDevCmd in that case.
    $env:PATH=$baseline+';'+((0..350|ForEach-Object {"C:\Atlas-CI-Path-Regression\$_"})-join ';')
    if($env:PATH.Length-le 8191){throw 'Long PATH fixture is insufficient'}
    . (Join-Path $PSScriptRoot 'initialize-msvc.ps1')
    $compiler=(Get-Command cl.exe -ErrorAction Stop).Source
    'int atlas_ci_contract(void) { return 242; }'|Set-Content -LiteralPath (Join-Path $fixture 'contract.c') -Encoding ascii
    & $compiler /nologo /c (Join-Path $fixture 'contract.c') (('/Fo'+(Join-Path $fixture 'contract.obj'))) *> (Join-Path $fixture 'compiler.log')
    if($LASTEXITCODE-ne 0 -or -not(Test-Path -LiteralPath (Join-Path $fixture 'contract.obj'))){throw 'Compiler is unusable after repeated/long-PATH initialization'}
    Write-Output 'PASS: repeated initialization is stable, CI markers exported, real MSVC compiles with PATH >8191.'
} finally {
    $env:PATH=$originalPath;$env:GITHUB_ENV=$originalExport
    $resolved=[IO.Path]::GetFullPath($fixture);$tempRoot=[IO.Path]::GetFullPath($env:TEMP).TrimEnd('\')+'\'
    if(-not $resolved.StartsWith($tempRoot,[StringComparison]::OrdinalIgnoreCase) -or (Split-Path $resolved -Leaf) -notlike 'atlas-msvc-acceptance-*'){throw 'Unsafe MSVC fixture cleanup'}
    Remove-Item -LiteralPath $resolved -Recurse -Force
}