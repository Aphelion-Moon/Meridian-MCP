param(
    [Parameter(Mandatory)][string]$BinaryPath,
    [Parameter(Mandatory)][string]$DreamMakerPath,
    [Parameter(Mandatory)][string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
Import-Module ./scripts/MeridianMcpSession.psm1 -Force
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Use a fresh output directory.' }
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
$output = (Resolve-Path -LiteralPath $OutputDirectory).Path
$workspace = Join-Path $output 'workspace'; $state = Join-Path $output 'state'
New-Item -ItemType Directory -Path $workspace,$state | Out-Null
$dme = Join-Path $workspace 'broken.dme'
[IO.File]::WriteAllText($dme,"/proc/fixture()`n    return meridian_missing_fixture_proc()`n")
$binary = (Resolve-Path -LiteralPath $BinaryPath).Path
$requests = @(
    (ConvertTo-McpJsonLine @{jsonrpc='2.0';id=1;method='initialize';params=@{protocolVersion='2025-11-25';capabilities=@{};clientInfo=@{name='native-diagnostic-audit';version='1'}}}),
    (ConvertTo-McpJsonLine @{jsonrpc='2.0';method='notifications/initialized';params=@{}}),
    (ConvertTo-McpJsonLine @{jsonrpc='2.0';id=2;method='tools/call';params=@{name='dm_compile';arguments=@{dme_path=$dme;timeout_ms=10000;idle_timeout_ms=5000;include_output=$false}}})
)
$session = Invoke-McpSession -BinaryPath $binary -WorkingDirectory (Get-Location).Path -Environment @{
    MERIDIAN_MCP_MODE='development';MERIDIAN_MCP_ROOTS=$workspace;MERIDIAN_MCP_STATE_DIR=$state;MERIDIAN_MCP_COMPILERS=$DreamMakerPath;
    MERIDIAN_MCP_RIFT_BUILD='disabled';MERIDIAN_MCP_TRACY='disabled';MERIDIAN_MCP_DEBUGGER='disabled'
} -Requests $requests -TimeoutMilliseconds 15000
[IO.File]::WriteAllText((Join-Path $output 'raw.json'),($session | ConvertTo-Json -Depth 30))
$response = (Get-McpResponse $session.Responses 2).result
$body = $response.content[0].text | ConvertFrom-Json
if ($session.ExitCode -ne 0 -or $body.success -ne $false -or -not $response.isError -or
    $body.diagnostic_summary.errors -lt 1 -or -not $body.diagnostic_summary.analysis_complete -or $body.dmb_exists) {
    throw 'Native invalid source did not return a complete failed compilation.'
}
$result = [ordered]@{
    binary_sha256=(Get-FileHash $binary).Hash.ToLowerInvariant();byond_version=(Get-Item $DreamMakerPath).VersionInfo.FileVersion;
    success=$body.success;compiler_exit_code=$body.exit_code;dmb_exists=$body.dmb_exists;
    diagnostic_summary=$body.diagnostic_summary;
    diagnostics=@($body.errors | ForEach-Object { @{file=[IO.Path]::GetFileName($_.file);line=$_.line;column=$_.column;severity=$_.severity;message=$_.message} });
    provenance_status=$body.provenance_status;natural_mcp_exit_code=$session.ExitCode
}
$text = $result | ConvertTo-Json -Depth 12
[IO.File]::WriteAllText((Join-Path $output 'summary.json'),$text)
$text
