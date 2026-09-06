[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$BinaryPath,
    [Parameter(Mandatory)][string]$HelperManifestPath
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0
Import-Module (Join-Path $PSScriptRoot 'MeridianMcpSession.psm1') -Force
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$fixture = Join-Path $repoRoot ('target/spacemandmm-docs-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
$stateName = '.meridian-spacemandmm-docs-state-' + [guid]::NewGuid().ToString('N')
$stateDirectory = Join-Path ([IO.Path]::GetTempPath()) $stateName
New-Item -ItemType Directory -Path $stateDirectory | Out-Null
$sources = [ordered]@{
    'fixture.dme' = "#include `"fixture.dm`"`n"
    'fixture.dm' = "/// SPACEMAN_AUDIT_TYPE`n/datum/audit_fixture`n`t/// SPACEMAN_AUDIT_MEMBER`n`tvar/charge = 7`n"
    'SpacemanDMM.toml' = "[dmdoc]`nindex_file = `"index.md`"`n"
    'index.md' = "# SPACEMAN_AUDIT_INDEX`n`nPurpose-written documentation fixture.`n"
}
foreach ($entry in $sources.GetEnumerator()) { [IO.File]::WriteAllText((Join-Path $fixture $entry.Key), $entry.Value) }
$before = @{}
foreach ($name in $sources.Keys) { $before[$name] = (Get-FileHash (Join-Path $fixture $name)).Hash }
$output = Join-Path $fixture 'html'
$requests = [System.Collections.Generic.List[string]]::new()
$requests.Add((ConvertTo-McpJsonLine @{ jsonrpc='2.0'; id=1; method='initialize'; params=@{protocolVersion='2024-11-05'; capabilities=@{}; clientInfo=@{name='spacemandmm-docs-test';version='1'}} }))
$requests.Add((ConvertTo-McpJsonLine @{ jsonrpc='2.0'; method='notifications/initialized'; params=@{} }))
foreach ($call in @(
    @(2, 'dm_parse_environment', @{dme_path=(Join-Path $fixture 'fixture.dme')}),
    @(3, 'dm_generate_docs', @{output_directory=$output}),
    @(4, 'dm_generate_docs', @{output_directory=$output})
)) {
    $requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=$call[0];method='tools/call';params=@{name=$call[1];arguments=$call[2]}}))
}
try {
    $session = Invoke-McpSession -BinaryPath $BinaryPath -WorkingDirectory $repoRoot -Environment @{
        MERIDIAN_MCP_MODE='development'
        MERIDIAN_MCP_ROOTS=$fixture
        MERIDIAN_MCP_HELPER_MANIFEST=(Resolve-Path -LiteralPath $HelperManifestPath).Path
        MERIDIAN_MCP_STATE_DIR=$stateDirectory
        MERIDIAN_MCP_RIFT_BUILD='disabled'
        MERIDIAN_MCP_TRACY='disabled'
        MERIDIAN_MCP_DEBUGGER='disabled'
    } -Requests $requests.ToArray() -TimeoutMilliseconds 120000
    if ($session.ExitCode -ne 0) { throw "Documentation MCP session exited with $($session.ExitCode)." }
    foreach ($id in @(2,3)) {
        $response = Get-McpResponse $session.Responses $id
        if ($response.result.isError) { throw "Documentation request $id failed: $($response.result.content[0].text)" }
    }
    $generated = (Get-McpResponse $session.Responses 3).result.content[0].text | ConvertFrom-Json
    if ($generated.source_revision -ne '351ddc0ffb2439876d4565ce5130bb6b027ee605') { throw 'Wrong dmdoc source revision' }
    if ($generated.files -lt 2 -or $generated.bytes -le 0) { throw 'dmdoc did not report populated output' }
    if (-not (Get-McpResponse $session.Responses 4).result.isError) { throw 'Existing documentation output was overwritten without opt-in' }
    $index = [IO.File]::ReadAllText((Join-Path $output 'index.html'))
    if (-not $index.Contains('SPACEMAN_AUDIT_INDEX')) { throw 'Configured dmdoc index was ignored' }
    $html = @(Get-ChildItem -LiteralPath $output -Recurse -Filter '*.html' | ForEach-Object { [IO.File]::ReadAllText($_.FullName) }) -join "`n"
    foreach ($marker in @('SPACEMAN_AUDIT_TYPE','SPACEMAN_AUDIT_MEMBER')) {
        if (-not $html.Contains($marker)) { throw "Generated docs omit $marker" }
    }
    foreach ($name in $sources.Keys) {
        if ((Get-FileHash (Join-Path $fixture $name)).Hash -ne $before[$name]) { throw "Documentation generation changed source $name" }
    }
    Write-Output "SpacemanDMM documentation stdio fixture passed: generated HTML, configured index, source preservation, overwrite rejection and clean MCP exit."
} finally {
    $resolvedState = (Resolve-Path -LiteralPath $stateDirectory).Path
    $expectedState = Join-Path (Resolve-Path -LiteralPath ([IO.Path]::GetTempPath())).Path $stateName
    if ($resolvedState -cne $expectedState) { throw 'Unexpected temporary state cleanup target' }
    Remove-Item -LiteralPath $resolvedState -Recurse -Force
}
