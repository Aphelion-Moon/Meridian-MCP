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
$modes = @('quiet','error','malformed','duplicate','cache','many','oversized','flood','missing')
$rows = @(); $sessions = @()
for ($round = 0; $round -lt $Repeats; $round++) {
    $order = if ($round % 2 -eq 0) { @('A','B') } else { @('B','A') }
    foreach ($arm in $order) {
        $run = Join-Path $output "round-$round-$arm"
        $workspace = Join-Path $run 'workspace'; $state = Join-Path $run 'state'
        New-Item -ItemType Directory -Path $workspace,$state -Force | Out-Null
        $requests = [System.Collections.Generic.List[string]]::new()
        $requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=1;method='initialize';params=@{protocolVersion='2025-11-25';capabilities=@{};clientInfo=@{name='rift-output-audit';version='1'}}}))
        $requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';method='notifications/initialized';params=@{}}))
        $requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=2;method='tools/call';params=@{name='dm_server_status';arguments=@{}}}))
        $requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=3;method='tools/list';params=@{}}))
        $specs = @($modes | ForEach-Object { @{mode=$_;options=@{};variant='default'} })
        if ($arm -eq 'B') {
            $specs += @{mode='flood';options=@{include_output=$false};variant='no-output'}
            foreach ($limit in @(0,2,200)) {
                $specs += @{mode='many';options=@{include_output=$false;diagnostic_limit=$limit};variant="limit-$limit"}
            }
        }
        $cases = @(); $id = 10
        foreach ($spec in $specs) {
            $project = Join-Path $workspace "$($spec.mode)-$($spec.variant)"
            New-Item -ItemType Directory -Path $project | Out-Null
            $dme = Join-Path $project 'tgstation.dme'
            [IO.File]::WriteAllText($dme,"/world`n    fps = 10`n")
            [IO.File]::WriteAllText((Join-Path $project 'BUILD.cmd'),"@echo off`r`n")
            [IO.File]::WriteAllText((Join-Path $project 'RIFT_BUILD.cmd'),"@echo off`r`n`"%DM_EXE%`"`r`nexit /b %ERRORLEVEL%`r`n")
            [IO.File]::WriteAllText((Join-Path $project 'dependencies.sh'),"export BYOND_MAJOR=516`nexport BYOND_MINOR=1687`n")
            [IO.File]::WriteAllText((Join-Path $project 'output-mode.txt'),$spec.mode)
            if ($spec.mode -eq 'cache') {
                [IO.File]::WriteAllBytes((Join-Path $project 'tgstation.dmb'),[Text.Encoding]::UTF8.GetBytes('rift fixture dmb'))
                [IO.File]::WriteAllBytes((Join-Path $project 'tgstation.rsc'),[Text.Encoding]::UTF8.GetBytes('rift fixture rsc'))
            }
            $requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=$id;method='tools/call';params=@{name='dm_parse_environment';arguments=@{dme_path=$dme}}}))
            $id++
            $arguments = @{timeout_ms=10000}
            foreach ($key in $spec.options.Keys) { $arguments[$key] = $spec.options[$key] }
            $request = ConvertTo-McpJsonLine @{jsonrpc='2.0';id=$id;method='tools/call';params=@{name='rift_compile';arguments=$arguments}}
            $requests.Add($request)
            $cases += @{id=$id;mode=$spec.mode;variant=$spec.variant;project=$project;request_bytes=[Text.Encoding]::UTF8.GetByteCount($request)}
            $id++
        }
        $session = Invoke-McpSession -BinaryPath $binaries[$arm] -WorkingDirectory (Get-Location).Path -Environment @{
            MERIDIAN_MCP_MODE='development';MERIDIAN_MCP_ROOTS=$workspace;MERIDIAN_MCP_STATE_DIR=$state;MERIDIAN_MCP_COMPILERS=$compiler;
            MERIDIAN_MCP_RIFT_BUILD='offline';MERIDIAN_MCP_TRACY='disabled';MERIDIAN_MCP_DEBUGGER='disabled'
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
        $tools = (Get-McpResponse $session.Responses 3).result.tools
        $riftTool = $tools | Where-Object name -eq 'rift_compile'
        $definitionBytes = [Text.Encoding]::UTF8.GetByteCount(($riftTool | ConvertTo-Json -Compress -Depth 25))
        $sessions += [ordered]@{arm=$arm;round=$round;exit_code=$session.ExitCode;build=$status.mcp_build;attempts=$attempts;tool_count=@($tools).Count;rift_definition_utf8_bytes=$definitionBytes}
        foreach ($case in $cases) {
            $parsed = (Get-McpResponse $session.Responses ($case.id-1)).result
            if ($parsed.isError) { throw "Fixture parse failed in $arm/$round/$($case.mode)" }
            $response = (Get-McpResponse $session.Responses $case.id).result
            $text = $response.content[0].text; $body = $text | ConvertFrom-Json
            $bodyBytes = [Text.Encoding]::UTF8.GetByteCount($text)
            $expectedSuccess = $case.mode -in @('quiet','flood','cache')
            $expectedErrors = switch ($case.mode) { 'error' {1} 'many' {30000} default {0} }
            $dmb = Join-Path $case.project 'tgstation.dmb'
            $rsc = Join-Path $case.project 'tgstation.rsc'
            $dmbHash = if (Test-Path -LiteralPath $dmb) { (Get-FileHash -LiteralPath $dmb).Hash.ToLowerInvariant() } else { $null }
            $rscHash = if (Test-Path -LiteralPath $rsc) { (Get-FileHash -LiteralPath $rsc).Hash.ToLowerInvariant() } else { $null }
            if ($arm -eq 'B') {
                if ($bodyBytes -gt 524288 -or $body.success -ne $expectedSuccess -or
                    $body.diagnostic_summary.errors -ne $expectedErrors -or
                    $body.diagnostic_summary.analysis_complete -ne ($case.mode -ne 'oversized') -or
                    -not $body.diagnostic_summary.output_complete -or $body.provenance_status -ne 'unverified' -or
                    $body.artifact_after.dmb.sha256 -ne $dmbHash -or $body.artifact_after.rsc.sha256 -ne $rscHash) {
                    throw "Candidate evidence mismatch in $($case.mode)/$($case.variant)"
                }
            }
            $rows += [ordered]@{
                arm=$arm;round=$round;mode=$case.mode;variant=$case.variant;request_id=$case.id;
                request_bytes=$case.request_bytes;response_text_utf8_bytes=$bodyBytes;wire_bytes=$wireBytes[$case.id];
                roundtrip_ms=$session.ResponseTimingsMilliseconds[[string]$case.id];build_duration_ms=$body.duration_ms;
                success=$body.success;code=$body.code;evidence=$body.evidence;is_error=[bool]$response.isError;
                artifact_dmb_sha256=$body.artifact_after.dmb.sha256;actual_dmb_sha256=$dmbHash;
                artifact_rsc_sha256=$body.artifact_after.rsc.sha256;actual_rsc_sha256=$rscHash;
                provenance_status=$body.provenance_status;has_build_record_id=($null -ne $body.build_record_id);
                diagnostic_summary=$body.diagnostic_summary;returned_diagnostics=$(if ($null -eq $body.diagnostics) { 0 } else { @($body.diagnostics).Count });
                first_diagnostic=$(if ($null -ne $body.diagnostics -and @($body.diagnostics).Count) { $body.diagnostics[0] } else { $null });
                cache_evidence=$body.cache_evidence;output_summary=$body.output_summary;
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
