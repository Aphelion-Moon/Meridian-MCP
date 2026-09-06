[CmdletBinding()]
param([Parameter(Mandatory)][string]$BinaryPath,[Parameter(Mandatory)][string]$DreamMakerPath,[Parameter(Mandatory)][string]$OutputDirectory)
$ErrorActionPreference='Stop'
$repo=(Get-Location).Path
Import-Module (Join-Path $repo 'scripts/MeridianMcpSession.psm1') -Force
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Use a fresh output directory.' }
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
$output=(Resolve-Path -LiteralPath $OutputDirectory).Path
$binary=Join-Path $output 'mcp.exe'
Copy-Item -LiteralPath (Resolve-Path -LiteralPath $BinaryPath).Path -Destination $binary
$stateName='.meridian-compiler-probe-'+[guid]::NewGuid().ToString('N')
$stateRoot=Join-Path ([IO.Path]::GetTempPath()) $stateName
New-Item -ItemType Directory -Path $stateRoot | Out-Null
$requests=[System.Collections.Generic.List[string]]::new()
$requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=1;method='initialize';params=@{protocolVersion='2025-11-25';capabilities=@{};clientInfo=@{name='compiler-native-audit';version='1'}}}))
$requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';method='notifications/initialized';params=@{}}))
$requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=2;method='tools/call';params=@{name='dm_server_status';arguments=@{}}}))
$cases=@('default','absolute_with_directory','relative_with_directory','invalid_defines')
$dmes=@()
for ($index=0;$index -lt $cases.Count;$index++) {
    $work=Join-Path $output ($cases[$index]+'/work directory')
    $project=Join-Path $work 'project'
    New-Item -ItemType Directory -Path $project -Force | Out-Null
    $dme=Join-Path $project 'fixture.dme'; $dmes+= $dme
    [IO.File]::WriteAllText($dme, '#include "fixture.dm"')
    [IO.File]::WriteAllText((Join-Path $project 'fixture.dm'), "world`n    fps = 10`n")
    $id=10+10*$index
    $requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=$id;method='tools/call';params=@{name='dm_parse_environment';arguments=@{dme_path=$dme}}}))
    $compileArguments=@{dme_path=$dme;timeout_ms=10000;idle_timeout_ms=5000}
    if ($cases[$index] -in @('absolute_with_directory','relative_with_directory')) { $compileArguments.working_directory=$work }
    if ($cases[$index] -eq 'relative_with_directory') { $compileArguments.dme_path='project/fixture.dme' }
    if ($cases[$index] -eq 'invalid_defines') { $compileArguments.defines=@('FLAG',42) }
    $requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=($id+1);method='tools/call';params=@{name='dm_compile';arguments=$compileArguments}}))
}
try {
    $session=Invoke-McpSession -BinaryPath $binary -WorkingDirectory $repo -Environment @{
        MERIDIAN_MCP_MODE='development';MERIDIAN_MCP_ROOTS=$output;MERIDIAN_MCP_COMPILERS=$DreamMakerPath;MERIDIAN_MCP_STATE_DIR=$stateRoot;
        MERIDIAN_MCP_RIFT_BUILD='disabled';MERIDIAN_MCP_TRACY='disabled';MERIDIAN_MCP_DEBUGGER='disabled'
    } -Requests $requests.ToArray() -TimeoutMilliseconds 60000
    [IO.File]::WriteAllText((Join-Path $output 'raw.json'),($session | ConvertTo-Json -Depth 45))
    $rows=@()
    for ($index=0;$index -lt $cases.Count;$index++) {
        $id=10+10*$index
        $parsed=(Get-McpResponse $session.Responses $id).result
        $response=(Get-McpResponse $session.Responses ($id+1)).result
        $body=$response.content[0].text | ConvertFrom-Json
        $rows+= [ordered]@{case=$cases[$index];parse_error=($parsed.isError -eq $true);is_error=($response.isError -eq $true);code=$body.code;success=$body.success;exit_code=$body.exit_code;dmb_exists=(Test-Path -LiteralPath ([IO.Path]::ChangeExtension($dmes[$index],'dmb')));dmb_updated=$body.dmb_updated;provenance_status=$body.provenance_status;defines=$body.defines}
    }
    $build=((Get-McpResponse $session.Responses 2).result.content[0].text | ConvertFrom-Json).mcp_build
    $result=[ordered]@{binary_sha256=(Get-FileHash $binary).Hash.ToLowerInvariant();byond_version=(Get-Item $DreamMakerPath).VersionInfo.FileVersion;build=$build;cases=$rows;exit_code=$session.ExitCode}
    $json=$result | ConvertTo-Json -Depth 12
    [IO.File]::WriteAllText((Join-Path $output 'summary.json'),$json)
    $json
    if ($session.ExitCode -ne 0) { throw 'MCP did not exit naturally.' }
} finally {
    $resolved=(Resolve-Path -LiteralPath $stateRoot).Path
    $expected=Join-Path (Resolve-Path -LiteralPath ([IO.Path]::GetTempPath())).Path $stateName
    if ($resolved -cne $expected) { throw 'Unexpected state cleanup target.' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
