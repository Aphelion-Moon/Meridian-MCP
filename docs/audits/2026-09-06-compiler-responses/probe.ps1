[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$BaselineBinaryPath,
    [Parameter(Mandatory)][string]$CandidateBinaryPath,
    [Parameter(Mandatory)][string]$CompilerPath,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [ValidateRange(1,10)][int]$Repeats = 3
)
$ErrorActionPreference = 'Stop'
Import-Module ./scripts/MeridianMcpSession.psm1 -Force
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Use a fresh output directory.' }
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
$output = (Resolve-Path -LiteralPath $OutputDirectory).Path
$compiler = (Resolve-Path -LiteralPath $CompilerPath).Path
$binaries = @{}
foreach ($entry in @(@('A',$BaselineBinaryPath),@('B',$CandidateBinaryPath))) {
    $destination = Join-Path $output ($entry[0]+[IO.Path]::GetExtension($entry[1]))
    Copy-Item -LiteralPath (Resolve-Path -LiteralPath $entry[1]).Path -Destination $destination
    $binaries[$entry[0]] = $destination
}
$modes = @('quiet','moderate','few_diagnostics','diagnostics','dual','unicode','controls','giant')
$rows = @(); $sessions = @()
for ($round = 0; $round -lt $Repeats; $round++) {
    $order = if ($round % 2 -eq 0) { @('A','B') } else { @('B','A') }
    foreach ($arm in $order) {
        $run = Join-Path $output "round-$round-$arm"
        $workspace = Join-Path $run 'workspace'; $state = Join-Path $run 'state'
        New-Item -ItemType Directory -Path $workspace,$state -Force | Out-Null
        $requests = [System.Collections.Generic.List[string]]::new()
        $requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=1;method='initialize';params=@{protocolVersion='2025-11-25';capabilities=@{};clientInfo=@{name='compiler-response-audit';version='1'}}}))
        $requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';method='notifications/initialized';params=@{}}))
        $requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=2;method='tools/list';params=@{}}))
        $requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=3;method='tools/call';params=@{name='dm_server_status';arguments=@{}}}))
        $cases = @(); $id = 10
        foreach ($mode in $modes) {
            $variants = if ($arm -eq 'A') { @('default') } else { @('default','no_output') }
            foreach ($variant in $variants) {
                $project = Join-Path $workspace "$mode-$variant"
                New-Item -ItemType Directory -Path $project | Out-Null
                $dme = Join-Path $project 'fixture.dme'
                [IO.File]::WriteAllText($dme,'// owned compiler output fixture')
                $compileArguments = @{dme_path=$dme;defines=@("OUTPUT_MODE=$mode");timeout_ms=10000;idle_timeout_ms=5000}
                if ($variant -eq 'no_output') { $compileArguments.include_output = $false }
                $request = ConvertTo-McpJsonLine @{jsonrpc='2.0';id=$id;method='tools/call';params=@{name='dm_compile';arguments=$compileArguments}}
                $requests.Add($request)
                $cases += @{id=$id;mode=$mode;variant=$variant;dme=$dme;request_bytes=[Text.Encoding]::UTF8.GetByteCount($request)}
                $id++
            }
        }
        $session = Invoke-McpSession -BinaryPath $binaries[$arm] -WorkingDirectory (Get-Location).Path -Environment @{
            MERIDIAN_MCP_MODE='development';MERIDIAN_MCP_ROOTS=$workspace;MERIDIAN_MCP_STATE_DIR=$state;MERIDIAN_MCP_COMPILERS=$compiler;
            MERIDIAN_MCP_RIFT_BUILD='disabled';MERIDIAN_MCP_TRACY='disabled';MERIDIAN_MCP_DEBUGGER='disabled'
        } -Requests $requests.ToArray() -TimeoutMilliseconds 60000
        [IO.File]::WriteAllText((Join-Path $run 'raw.json'),($session | ConvertTo-Json -Depth 40))
        if ($session.ExitCode -ne 0) { throw "MCP failed to shut down in $arm/$round" }
        $wireBytes = @{}
        foreach ($line in ($session.Stdout -split '\r?\n')) {
            if (-not $line.Trim()) { continue }
            $message = $line | ConvertFrom-Json
            if ($null -ne $message.id) { $wireBytes[[int]$message.id] = [Text.Encoding]::UTF8.GetByteCount($line) }
        }
        $status = (Get-McpResponse $session.Responses 3).result.content[0].text | ConvertFrom-Json
        $definition = @((Get-McpResponse $session.Responses 2).result.tools | Where-Object name -eq 'dm_compile')[0]
        $sessions += [ordered]@{
            arm=$arm;round=$round;exit_code=$session.ExitCode;build=$status.mcp_build;
            compiler_definition_bytes=[Text.Encoding]::UTF8.GetByteCount(($definition | ConvertTo-Json -Depth 20 -Compress));
            tools_list_wire_bytes=$wireBytes[2]
        }
        foreach ($case in $cases) {
            $response = (Get-McpResponse $session.Responses $case.id).result
            $text = $response.content[0].text
            $body = $text | ConvertFrom-Json
            $hasStatus = $null -ne $body.PSObject.Properties['success']
            $errors = if ($null -ne $body.diagnostic_summary) { $body.diagnostic_summary.errors } elseif ($hasStatus) { @($body.errors).Count } else { $null }
            $warnings = if ($null -ne $body.diagnostic_summary) { $body.diagnostic_summary.warnings } elseif ($hasStatus) { @($body.warnings).Count } else { $null }
            $expectedErrors = switch ($case.mode) { 'few_diagnostics' {20} 'diagnostics' {600} 'giant' {1} default {0} }
            $expectedWarnings = switch ($case.mode) { 'few_diagnostics' {10} 'diagnostics' {10} 'giant' {1} default {0} }
            $dmb = [IO.Path]::ChangeExtension($case.dme,'dmb')
            $actualHash = (Get-FileHash -LiteralPath $dmb).Hash.ToLowerInvariant()
            $bodyBytes = [Text.Encoding]::UTF8.GetByteCount($text)
            if ($arm -eq 'B') {
                if (-not $hasStatus -or $bodyBytes -gt 524288 -or $body.success -ne ($expectedErrors -eq 0) -or
                    $errors -ne $expectedErrors -or $warnings -ne $expectedWarnings -or
                    -not $body.dmb_exists -or $body.artifact_after.sha256 -ne $actualHash -or $body.provenance_status -ne 'unverified') {
                    throw "Candidate result lost evidence in $($case.mode)/$($case.variant)"
                }
            }
            $rows += [ordered]@{
                arm=$arm;round=$round;mode=$case.mode;variant=$case.variant;
                request_bytes=$case.request_bytes;response_text_utf8_bytes=$bodyBytes;response_text_utf16_units=$text.Length;
                wire_bytes=$wireBytes[$case.id];roundtrip_ms=$session.ResponseTimingsMilliseconds[[string]$case.id];
                code=$body.code;overflow_output_bytes=$body.details.output_bytes;
                has_compile_status=$hasStatus;success=$body.success;is_error=[bool]$response.isError;
                artifact_sha256=$body.artifact_after.sha256;actual_artifact_sha256=$actualHash;provenance_status=$body.provenance_status;
                errors=$errors;warnings=$warnings;
                returned_errors=$(if ($hasStatus) { @($body.errors).Count } else { $null });
                returned_warnings=$(if ($hasStatus) { @($body.warnings).Count } else { $null });
                diagnostic_summary=$body.diagnostic_summary;output_summary=$body.output_summary;
                stdout_capture_truncated_bytes=$body.stdout_truncated_bytes;stderr_capture_truncated_bytes=$body.stderr_truncated_bytes
            }
        }
    }
}
$result = [ordered]@{
    schema=1;repeats=$Repeats;arm_A='baseline';arm_B='candidate';
    baseline_sha256=(Get-FileHash $binaries.A).Hash.ToLowerInvariant();candidate_sha256=(Get-FileHash $binaries.B).Hash.ToLowerInvariant();
    fixture_sha256=(Get-FileHash $compiler).Hash.ToLowerInvariant();sessions=$sessions;cases=$rows
}
[IO.File]::WriteAllText((Join-Path $output 'summary.json'),($result | ConvertTo-Json -Depth 25))
[pscustomobject]@{cases=$rows.Count;sessions=$sessions.Count;candidate_failures=0;summary='summary.json'} | ConvertTo-Json
