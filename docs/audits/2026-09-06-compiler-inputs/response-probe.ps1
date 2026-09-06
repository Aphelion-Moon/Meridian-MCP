param([Parameter(Mandatory)][string]$BinaryPath,[Parameter(Mandatory)][string]$OutputDirectory)
$ErrorActionPreference='Stop'
Import-Module ./scripts/MeridianMcpSession.psm1 -Force
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Use a fresh output directory.' }
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
$output=(Resolve-Path -LiteralPath $OutputDirectory).Path
$workspace=Join-Path $output 'workspace'; $state=Join-Path $output 'state'
New-Item -ItemType Directory -Path $workspace,$state | Out-Null
$dme=Join-Path $workspace 'fixture.dme'
[IO.File]::WriteAllText($dme,'// response budget fixture')
$compiler=(Resolve-Path -LiteralPath 'target/compiler-output-flood.exe').Path
$binary=(Resolve-Path -LiteralPath $BinaryPath).Path
$requests=@(
    (ConvertTo-McpJsonLine @{jsonrpc='2.0';id=1;method='initialize';params=@{protocolVersion='2025-11-25';capabilities=@{};clientInfo=@{name='compile-output-audit';version='1'}}}),
    (ConvertTo-McpJsonLine @{jsonrpc='2.0';method='notifications/initialized';params=@{}}),
    (ConvertTo-McpJsonLine @{jsonrpc='2.0';id=2;method='tools/call';params=@{name='dm_compile';arguments=@{dme_path=$dme;timeout_ms=10000;idle_timeout_ms=5000}}})
)
$session=Invoke-McpSession -BinaryPath $binary -WorkingDirectory (Get-Location).Path -Environment @{
    MERIDIAN_MCP_MODE='development';MERIDIAN_MCP_ROOTS=$workspace;MERIDIAN_MCP_STATE_DIR=$state;MERIDIAN_MCP_COMPILERS=$compiler;
    MERIDIAN_MCP_RIFT_BUILD='disabled';MERIDIAN_MCP_TRACY='disabled';MERIDIAN_MCP_DEBUGGER='disabled'
} -Requests $requests -TimeoutMilliseconds 15000
[IO.File]::WriteAllText((Join-Path $output 'raw.json'),($session | ConvertTo-Json -Depth 25))
$response=(Get-McpResponse $session.Responses 2).result
$body=$response.content[0].text | ConvertFrom-Json
$result=[ordered]@{binary_sha256=(Get-FileHash $binary).Hash.ToLowerInvariant();requested_stdout_bytes=600000;requested_stderr_bytes=600000;code=$body.code;dmb_created=(Test-Path -LiteralPath ([IO.Path]::ChangeExtension($dme,'dmb')));compile_status_returned=($null -ne $body.success);reply=@{code=$body.code;details=$body.details;message=$body.message;recovery=$body.recovery};exit_code=$session.ExitCode}
$json=$result | ConvertTo-Json -Depth 10
[IO.File]::WriteAllText((Join-Path $output 'summary.json'),$json)
$json
