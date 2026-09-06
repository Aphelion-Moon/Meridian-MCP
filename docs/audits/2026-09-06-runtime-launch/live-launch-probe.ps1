[CmdletBinding()]
param([Parameter(Mandatory)][string]$BinaryPath,[Parameter(Mandatory)][string]$OutputDirectory,[Parameter(Mandatory)][string]$DreamMakerPath)
$ErrorActionPreference='Stop'
$repoRoot=(Get-Location).Path
Import-Module (Join-Path $repoRoot 'scripts/MeridianMcpSession.psm1') -Force
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Use a fresh output directory.' }
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
$output=(Resolve-Path -LiteralPath $OutputDirectory).Path
$binary=Join-Path $output 'mcp.exe'
Copy-Item -LiteralPath (Resolve-Path -LiteralPath $BinaryPath).Path -Destination $binary
$artifact=Join-Path $output 'artifacts'
$requested=Join-Path $output 'requested directory'
New-Item -ItemType Directory -Path $artifact,$requested | Out-Null
[IO.File]::WriteAllText((Join-Path $artifact 'fixture.dme'), '#include "fixture.dm"')
[IO.File]::WriteAllText((Join-Path $artifact 'fixture.dm'), @'
world
    sleep_offline = 0
world/New()
    ..()
    spawn(2)
        world.log << "CWD_PROBE:[file2text("location.txt")]"
        world.log << "PARAM_PROBE:[list2params(world.params)]"
        world.log << "PROBE_READY"
    spawn(300)
        shutdown()
world/Topic(T, Addr, Master, Key)
    if(T == "ping")
        return "pong"
    return ..()
'@)
[IO.File]::WriteAllText((Join-Path $artifact 'location.txt'),'artifact')
[IO.File]::WriteAllText((Join-Path $requested 'location.txt'),'requested')
$compiler=Start-Process -FilePath $DreamMakerPath -ArgumentList 'fixture.dme' -WorkingDirectory $artifact -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $output 'compiler.out') -RedirectStandardError (Join-Path $output 'compiler.err')
if (-not $compiler.WaitForExit(30000)) { $compiler.Kill(); $compiler.WaitForExit(); throw 'Fixture compile timed out.' }
if ($compiler.ExitCode -ne 0 -or -not (Test-Path (Join-Path $artifact 'fixture.dmb'))) { throw 'Fixture compile failed; inspect compiler.out and compiler.err.' }
$stateName='.meridian-live-launch-'+[guid]::NewGuid().ToString('N')
$stateRoot=Join-Path ([IO.Path]::GetTempPath()) $stateName
New-Item -ItemType Directory -Path $stateRoot | Out-Null
$requests=[System.Collections.Generic.List[string]]::new()
$requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=1;method='initialize';params=@{protocolVersion='2025-11-25';capabilities=@{};clientInfo=@{name='live-launch-audit';version='1'}}}))
$requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';method='notifications/initialized';params=@{}}))
$requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=2;method='tools/call';params=@{name='dm_server_status';arguments=@{}}}))
$caseIds=@{}
foreach ($case in @(@('default',10),@('absolute_with_directory',20),@('relative_with_directory',30),@('parameter_values',40))) {
    $listener=[Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,0)
    $listener.Start(); $port=$listener.LocalEndpoint.Port; $listener.Stop()
    $arguments=@{dmb_path=(Join-Path $artifact 'fixture.dmb');port=$port;wait_for='PROBE_READY';startup_timeout_ms=5000}
    if ($case[0] -in @('absolute_with_directory','relative_with_directory')) { $arguments.working_directory=$requested }
    if ($case[0] -eq 'relative_with_directory') { $arguments.dmb_path='../artifacts/fixture.dmb' }
    if ($case[0] -eq 'parameter_values') { $arguments.daemon_args=@('-params','-ip','-params','9999','-params','note=-ip 0.0.0.0&file=fixture.dmb','-params inline=fixture.dmb') }
    $id=[int]$case[1]; $caseIds[[string]$id]=$case[0]
    foreach ($call in @(@($id,'dm_run',$arguments),@(($id+1),'dm_topic',@{topic='ping';timeout_ms=5000}),@(($id+2),'dm_stop',@{}))) {
        $requests.Add((ConvertTo-McpJsonLine @{jsonrpc='2.0';id=$call[0];method='tools/call';params=@{name=$call[1];arguments=$call[2]}}))
    }
}
$rows=[System.Collections.Generic.List[object]]::new()
try {
    $session=Invoke-McpSession -BinaryPath $binary -WorkingDirectory $repoRoot -Environment @{
        MERIDIAN_MCP_MODE='development';MERIDIAN_MCP_ROOTS=$output;MERIDIAN_MCP_COMPILERS=$DreamMakerPath;MERIDIAN_MCP_STATE_DIR=$stateRoot;
        MERIDIAN_MCP_RIFT_BUILD='disabled';MERIDIAN_MCP_TRACY='disabled';MERIDIAN_MCP_DEBUGGER='disabled'
    } -Requests $requests.ToArray() -TimeoutMilliseconds 45000 -AfterResponse {
        param($request,$response)
        if (-not $caseIds.ContainsKey([string]$request.id)) { return }
        $body=$response.result.content[0].text | ConvertFrom-Json
        $row=[ordered]@{case=$caseIds[[string]$request.id];success=($body.success -eq $true);error_code=$body.code;observed_cwd=$null;reported_directory=$null;listener_addresses=@();observed_parameters=$null}
        if ($row.success) {
            $row.reported_directory=if ($body.working_directory -eq $requested) {'requested'} elseif ($body.working_directory -eq $artifact) {'artifact'} else {'other'}
            foreach ($line in $body.readiness.recent_output) { if ($line -match 'CWD_PROBE:(artifact|requested)') { $row.observed_cwd=$Matches[1] } }
            foreach ($line in $body.readiness.recent_output) { if ($line -match '^PARAM_PROBE:(.*)$') { $row.observed_parameters=$Matches[1] } }
            $connections=@(Get-NetTCPConnection -OwningProcess $body.pid -State Listen -ErrorAction SilentlyContinue)
            $row.listener_addresses=@($connections | ForEach-Object { if ($_.LocalAddress -in @('127.0.0.1','::1','0.0.0.0','::')) { $_.LocalAddress } else { '<non-loopback>' } } | Sort-Object -Unique)
        }
        $rows.Add([pscustomobject]$row)
    }
    [IO.File]::WriteAllText((Join-Path $output 'raw-stdio.json'),($session | ConvertTo-Json -Depth 40))
    $build=((Get-McpResponse $session.Responses 2).result.content[0].text | ConvertFrom-Json).mcp_build
    $result=[ordered]@{probe_schema=2;fixture_source_sha256=(Get-FileHash (Join-Path $artifact 'fixture.dm')).Hash.ToLowerInvariant();binary_sha256=(Get-FileHash $binary).Hash.ToLowerInvariant();byond_version=(Get-Item $DreamMakerPath).VersionInfo.FileVersion;build=$build;cases=$rows.ToArray();exit_code=$session.ExitCode}
    foreach ($index in @(0,1,2,3)) {
        $id=10+($index*10)
        $topic=(Get-McpResponse $session.Responses ($id+1)).result
        $stop=(Get-McpResponse $session.Responses ($id+2)).result
        $rows[$index] | Add-Member -NotePropertyName topic_error -NotePropertyValue ($topic.isError -eq $true)
        $rows[$index] | Add-Member -NotePropertyName stop_error -NotePropertyValue ($stop.isError -eq $true)
    }
    $json=$result | ConvertTo-Json -Depth 12
    [IO.File]::WriteAllText((Join-Path $output 'summary.json'),$json)
    Write-Output $json
    if ($session.ExitCode -ne 0) { throw 'MCP did not exit naturally.' }
} finally {
    $resolved=(Resolve-Path -LiteralPath $stateRoot).Path
    $expected=Join-Path (Resolve-Path -LiteralPath ([IO.Path]::GetTempPath())).Path $stateName
    if ($resolved -cne $expected) { throw 'Unexpected state cleanup target.' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
