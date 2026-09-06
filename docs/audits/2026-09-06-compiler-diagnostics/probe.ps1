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
$modes = @('quiet','early_error','many_diagnostics','overlong_line')
$rows = @(); $sessions = @()
for ($round = 0; $round -lt $Repeats; $round++) {
    $order = if ($round % 2 -eq 0) { @('A','B') } else { @('B','A') }
    foreach ($arm in $order) {
        $run = Join-Path $output "round-$round-$arm"
        $workspace = Join-Path $run 'workspace'; $state = Join-Path $run 'state'
        New-Item -ItemType Directory -Path $workspace,$state -Force | Out-Null
        $requests = [System.Collections.Generic.List[string]]::new()
        $requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=1;method='initialize';params=@{protocolVersion='2025-11-25';capabilities=@{};clientInfo=@{name='compiler-diagnostic-audit';version='1'}}}))
        $requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';method='notifications/initialized';params=@{}}))
        $requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=2;method='tools/call';params=@{name='dm_server_status';arguments=@{}}}))
        $cases = @(); $id = 10
        foreach ($mode in $modes) {
            foreach ($limit in @(0,2,200)) {
                $project = Join-Path $workspace "$mode-$limit"
                New-Item -ItemType Directory -Path $project | Out-Null
                $dme = Join-Path $project "$mode.dme"
                [IO.File]::WriteAllText($dme,"/world`n    fps = 10`n")
                $requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=$id;method='tools/call';params=@{name='dm_parse_environment';arguments=@{dme_path=$dme}}}))
                $id++
                $compileArguments = @{dme_path=$dme;timeout_ms=10000;idle_timeout_ms=5000;include_output=$false;diagnostic_limit=$limit}
                $request = ConvertTo-McpJsonLine @{jsonrpc='2.0';id=$id;method='tools/call';params=@{name='dm_compile';arguments=$compileArguments}}
                $requests.Add($request)
                $cases += @{id=$id;mode=$mode;limit=$limit;dme=$dme;request_bytes=[Text.Encoding]::UTF8.GetByteCount($request)}
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
        $status = (Get-McpResponse $session.Responses 2).result.content[0].text | ConvertFrom-Json
        $attempts = @(Get-ChildItem -LiteralPath (Join-Path $state 'attempts') -Filter '*.json' | ForEach-Object { (Get-Content -LiteralPath $_.FullName -Raw | ConvertFrom-Json).outcome })
        $sessions += [ordered]@{arm=$arm;round=$round;exit_code=$session.ExitCode;build=$status.mcp_build;attempts=$attempts}
        foreach ($case in $cases) {
            $parsed = (Get-McpResponse $session.Responses ($case.id-1)).result
            if ($parsed.isError) { throw "Fixture parse failed in $arm/$round/$($case.mode)" }
            $response = (Get-McpResponse $session.Responses $case.id).result
            $text = $response.content[0].text
            $body = $text | ConvertFrom-Json
            $expectedErrors = switch ($case.mode) { 'early_error' {1} 'many_diagnostics' {30000} default {0} }
            $expectedWarnings = switch ($case.mode) { 'early_error' {1} 'many_diagnostics' {20000} default {0} }
            $expectedSuccess = $case.mode -eq 'quiet'
            $expectedProvenance = if ($expectedSuccess) { 'verified' } else { 'unverified' }
            $dmb = [IO.Path]::ChangeExtension($case.dme,'dmb')
            $actualHash = (Get-FileHash -LiteralPath $dmb).Hash.ToLowerInvariant()
            $bodyBytes = [Text.Encoding]::UTF8.GetByteCount($text)
            if ($arm -eq 'B') {
                if ($bodyBytes -gt 524288 -or $body.success -ne $expectedSuccess -or
                    $body.diagnostic_summary.errors -ne $expectedErrors -or $body.diagnostic_summary.warnings -ne $expectedWarnings -or
                    $body.diagnostic_summary.analysis_complete -ne ($case.mode -ne 'overlong_line') -or -not $body.diagnostic_summary.output_complete -or
                    -not $body.dmb_exists -or $body.artifact_after.sha256 -ne $actualHash -or $body.provenance_status -ne $expectedProvenance) {
                    throw "Candidate result lost evidence in $($case.mode)/$($case.limit)"
                }
            }
            $rows += [ordered]@{
                arm=$arm;round=$round;mode=$case.mode;limit=$case.limit;request_id=$case.id;
                request_bytes=$case.request_bytes;response_text_utf8_bytes=$bodyBytes;wire_bytes=$wireBytes[$case.id];
                roundtrip_ms=$session.ResponseTimingsMilliseconds[[string]$case.id];compile_duration_ms=$body.duration_ms;
                success=$body.success;is_error=[bool]$response.isError;artifact_sha256=$body.artifact_after.sha256;actual_artifact_sha256=$actualHash;
                provenance_status=$body.provenance_status;has_build_record_id=($null -ne $body.build_record_id);provenance_reasons=@($body.provenance_reasons.code);
                diagnostic_summary=$body.diagnostic_summary;diagnostic_analysis_error=$body.diagnostic_analysis_error;
                returned_errors=@($body.errors).Count;returned_warnings=@($body.warnings).Count;
                first_error_line=$(if (@($body.errors).Count) { $body.errors[0].line } else { $null });
                output_summary=$body.output_summary;stdout_capture_truncated_bytes=$body.stdout_truncated_bytes;stderr_capture_truncated_bytes=$body.stderr_truncated_bytes
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
