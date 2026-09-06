[CmdletBinding()]
param([Parameter(Mandatory)][string]$BinaryPath,[Parameter(Mandatory)][string]$DreamMakerPath,[Parameter(Mandatory)][string]$OutputDirectory)
$ErrorActionPreference = 'Stop'
Import-Module ./scripts/MeridianMcpSession.psm1 -Force
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Use a fresh output directory.' }
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
$output = (Resolve-Path -LiteralPath $OutputDirectory).Path
$workspace = Join-Path $output 'workspace'; $state = Join-Path $output 'state'
New-Item -ItemType Directory -Path $workspace,$state | Out-Null
$binary = Join-Path $output 'mcp.exe'
Copy-Item -LiteralPath (Resolve-Path -LiteralPath $BinaryPath).Path -Destination $binary
$requests = [System.Collections.Generic.List[string]]::new()
$requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=1;method='initialize';params=@{protocolVersion='2025-11-25';capabilities=@{};clientInfo=@{name='rift-native-audit';version='1'}}}))
$requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';method='notifications/initialized';params=@{}}))
$requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=2;method='tools/call';params=@{name='dm_server_status';arguments=@{}}}))
$cases = @('success','error'); $id=10
foreach ($case in $cases) {
    $project = Join-Path $workspace $case
    New-Item -ItemType Directory -Path $project | Out-Null
    [IO.File]::WriteAllText((Join-Path $project 'tgstation.dme'),'#include "fixture.dm"')
    $source = "/world`n    fps = 10`n/var/global/audit_resource = 'resource.txt'`n"
    if ($case -eq 'error') { $source += "/proc/audit_error()`n    missing_audit_proc()`n" }
    [IO.File]::WriteAllText((Join-Path $project 'fixture.dm'),$source)
    [IO.File]::WriteAllText((Join-Path $project 'resource.txt'),'owned audit resource')
    [IO.File]::WriteAllText((Join-Path $project 'BUILD.cmd'),"@echo off`r`n")
    [IO.File]::WriteAllText((Join-Path $project 'dependencies.sh'),"export BYOND_MAJOR=516`nexport BYOND_MINOR=1687`n")
    [IO.File]::WriteAllText((Join-Path $project 'RIFT_BUILD.cmd'),"@echo off`r`n`"%DM_EXE%`" tgstation.dme`r`nset `"build_exit=%ERRORLEVEL%`"`r`nfor /L %%i in (1,1,30000) do @echo ordinary build progress`r`nexit /b %build_exit%`r`n")
    $requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=$id;method='tools/call';params=@{name='dm_parse_environment';arguments=@{dme_path=(Join-Path $project 'tgstation.dme')}}}))
    $requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=($id+1);method='tools/call';params=@{name='rift_compile';arguments=@{timeout_ms=30000;include_output=$false}}}))
    $id += 10
}
$session = Invoke-McpSession -BinaryPath $binary -WorkingDirectory (Get-Location).Path -Environment @{
    MERIDIAN_MCP_MODE='development';MERIDIAN_MCP_ROOTS=$workspace;MERIDIAN_MCP_STATE_DIR=$state;MERIDIAN_MCP_COMPILERS=$DreamMakerPath;
    MERIDIAN_MCP_RIFT_BUILD='offline';MERIDIAN_MCP_TRACY='disabled';MERIDIAN_MCP_DEBUGGER='disabled'
} -Requests $requests.ToArray() -TimeoutMilliseconds 90000
[IO.File]::WriteAllText((Join-Path $output 'raw.json'),($session | ConvertTo-Json -Depth 40))
$rows = @(); $id=10
foreach ($case in $cases) {
    $parse = (Get-McpResponse $session.Responses $id).result
    $response = (Get-McpResponse $session.Responses ($id+1)).result
    $body = $response.content[0].text | ConvertFrom-Json
    $project = Join-Path $workspace $case
    $hashes = @{}
    foreach ($kind in @('dmb','rsc')) {
        $path = Join-Path $project ('tgstation.'+$kind)
        $hashes[$kind] = if (Test-Path -LiteralPath $path) { (Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant() } else { $null }
    }
    if ($parse.isError -or $body.success -ne ($case -eq 'success') -or
        $body.provenance_status -ne 'unverified' -or -not $body.diagnostic_summary.analysis_complete -or
        $body.artifact_after.dmb.sha256 -ne $hashes.dmb -or $body.artifact_after.rsc.sha256 -ne $hashes.rsc) {
        throw "Native wrapper evidence mismatch: $case"
    }
    $rows += [ordered]@{case=$case;success=$body.success;code=$body.code;is_error=[bool]$response.isError;
        evidence=$body.evidence;exit_code=$body.exit_code;provenance_status=$body.provenance_status;
        dmb_sha256=$hashes.dmb;rsc_sha256=$hashes.rsc;diagnostic_summary=$body.diagnostic_summary;
        diagnostics=@($body.diagnostics);response_text_utf8_bytes=[Text.Encoding]::UTF8.GetByteCount($response.content[0].text);
        stdout_capture_truncated_bytes=$body.stdout_truncated_bytes;output_summary=$body.output_summary}
    $id += 10
}
$attempts = @(Get-ChildItem -LiteralPath (Join-Path $state 'attempts') -Filter '*.json' | ForEach-Object { (Get-Content -LiteralPath $_.FullName -Raw | ConvertFrom-Json).outcome })
$result = [ordered]@{schema=1;binary_sha256=(Get-FileHash $binary).Hash.ToLowerInvariant();
    byond_version=(Get-Item -LiteralPath $DreamMakerPath).VersionInfo.FileVersion;natural_mcp_exit_code=$session.ExitCode;
    build=((Get-McpResponse $session.Responses 2).result.content[0].text | ConvertFrom-Json).mcp_build;cases=$rows;attempts=$attempts}
[IO.File]::WriteAllText((Join-Path $output 'summary.json'),($result | ConvertTo-Json -Depth 20))
if ($session.ExitCode -ne 0) { throw 'MCP did not exit naturally.' }
[pscustomobject]@{cases=$rows.Count;natural_mcp_exit_code=$session.ExitCode;summary='summary.json'} | ConvertTo-Json
