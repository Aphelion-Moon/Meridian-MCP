[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$DreamMakerPath,
    [Parameter(Mandatory)][string]$BinaryPath,
    [Parameter(Mandatory)][string]$EvidencePath
)
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'Native memory integration is Windows-only.' }
Import-Module (Join-Path $PSScriptRoot 'MeridianMcpSession.psm1') -Force
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$compiler = (Resolve-Path -LiteralPath $DreamMakerPath).Path
$binary = (Resolve-Path -LiteralPath $BinaryPath).Path
$evidence = [IO.Path]::GetFullPath($EvidencePath)
New-Item -ItemType Directory -Force (Split-Path -Parent $evidence) | Out-Null
$fixtureRoot = Join-Path $repo ('target/native-memory-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixtureRoot | Out-Null
Copy-Item (Join-Path $repo 'tests/fixtures/auxtools/auxtools.dm') (Join-Path $fixtureRoot 'auxtools.dm')
Copy-Item (Join-Path $repo 'tests/fixtures/memory/memory.dm') (Join-Path $fixtureRoot 'memory.dm')
[IO.File]::WriteAllText((Join-Path $fixtureRoot 'memory.dme'), "#include `"auxtools.dm`"`n#include `"memory.dm`"`n")
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$vs = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath).Trim()
Import-Module (Join-Path $vs 'Common7/Tools/Microsoft.VisualStudio.DevShell.dll')
Enter-VsDevShell -VsInstallPath $vs -SkipAutomaticLocation -DevCmdArguments '-arch=x86 -host_arch=x64'
& cl /nologo /LD /MD /O2 /W4 (Join-Path $repo 'tests/fixtures/memory/allocation_fixture.c') "/Fo$fixtureRoot/allocation_fixture.obj" /link "/OUT:$fixtureRoot/allocation_fixture.dll" "/IMPLIB:$fixtureRoot/allocation_fixture.lib"
if ($LASTEXITCODE -ne 0) { throw 'Native allocation fixture compile failed.' }
& $compiler (Join-Path $fixtureRoot 'memory.dme')
if ($LASTEXITCODE -ne 0) { throw 'Owned DM fixture compile failed.' }
$state = Join-Path ([IO.Path]::GetTempPath()) ('meridian-memory-integration-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $state | Out-Null
function Request($id,$name,$arguments) { ConvertTo-McpJsonLine @{jsonrpc='2.0';id=$id;method='tools/call';params=@{name=$name;arguments=$arguments}} }
function Eval($id,$expression) { Request $id 'dm_debug_evaluate' @{expression=$expression;context='repl'} }
$requests = @(
 (ConvertTo-McpJsonLine @{jsonrpc='2.0';id=1;method='initialize';params=@{protocolVersion='2024-11-05';capabilities=@{};clientInfo=@{name='native-memory-integration';version='1'}}}),
 (ConvertTo-McpJsonLine @{jsonrpc='2.0';method='notifications/initialized';params=@{}}),
 (Request 2 'dm_debug_launch' @{dmb_path=(Join-Path $fixtureRoot 'memory.dmb');host_mode='headless';memory_profile=$true}),
 (Request 3 'dm_debug_memory' @{action='status'}),
 (Eval 4 '#mem_profiler begin forbidden.txt'),
 (Request 5 'dm_debug_memory' @{action='start';max_records=100000}),
 (Request 6 'dm_debug_memory' @{action='start'}),
 (Eval 7 'memory_native_hold()'),
 (Request 8 'dm_debug_memory' @{action='stop'}),
 (Eval 9 'memory_native_release()'),
 (Request 10 'dm_debug_memory' @{action='start';max_records=100000}),
 (Eval 11 'memory_native_hold()'),
 (Eval 12 'memory_native_release()'),
 (Request 13 'dm_debug_memory' @{action='stop'}),
 (Request 14 'dm_debug_memory' @{action='start';max_records=1}),
 (Eval 15 'memory_allocate()'),
 (Request 16 'dm_debug_memory' @{action='stop'}),
 (Eval 17 'memory_release()'),
 (Request 18 'dm_debug_memory' @{action='start';duration_ms=1}),
 (Request 19 'dm_debug_memory' @{action='status'}),
 (Request 20 'dm_debug_memory' @{action='stop'}),
 (Request 21 'dm_debug_stop' @{}),
 (Request 22 'dm_debug_launch' @{dmb_path=(Join-Path $fixtureRoot 'memory.dmb');host_mode='headless'}),
 (Request 23 'dm_debug_memory' @{action='status'}),
 (Request 24 'dm_debug_stop' @{})
)
$result = Invoke-McpSession -BinaryPath $binary -WorkingDirectory $fixtureRoot -Environment @{
 MERIDIAN_MCP_MODE='development';MERIDIAN_MCP_ROOTS=$fixtureRoot;MERIDIAN_MCP_STATE_DIR=$state;MERIDIAN_MCP_COMPILERS=$compiler;MERIDIAN_MCP_DEBUGGER='auxtools'
} -Requests $requests -TimeoutMilliseconds 90000 -AfterResponse {
 param($request,$response)
 if ($request.id -eq 18) { Start-Sleep -Milliseconds 25 }
}
$result | ConvertTo-Json -Depth 40 | Set-Content -LiteralPath $evidence
function Payload($id) { ((Get-McpResponse -Responses $result.Responses -Id $id).result.content[0].text | ConvertFrom-Json) }
foreach ($id in @(2,3,5,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,24)) {
 $response = Get-McpResponse -Responses $result.Responses -Id $id
 if ($response.result.isError) { throw "Request $id failed: $($response.result.content[0].text)" }
}
foreach ($id in @(4,6,23)) {
 if (-not (Get-McpResponse -Responses $result.Responses -Id $id).result.isError) { throw "Expected request $id to fail closed." }
}
foreach ($id in @(7,11)) { if ((Payload $id).result.value -ne '128') { throw "Native allocation/realloc fixture failed: request $id" } }
foreach ($id in @(9,12)) { if ((Payload $id).result.value -ne '0') { throw "Native release fixture failed: request $id" } }
$held = (Payload 8).native_memory.evidence.result
$released = (Payload 13).native_memory.evidence.result
$heldProc = @($held.procedures | Where-Object proc_path -eq '/memory_native_hold')
$releasedBytes = [long](($released.procedures | Where-Object proc_path -eq '/memory_native_hold' | Measure-Object outstanding_requested_bytes -Sum).Sum)
if ($heldProc.Count -ne 1 -or $heldProc[0].outstanding_requested_bytes -lt 528384) { throw 'Known 528384 native bytes were not attributed.' }
if ($releasedBytes -ge 4096) { throw 'Released native blocks remain attributed.' }
if ($held.capacity_exceeded -or $released.capacity_exceeded) { throw 'Known-allocation qualification exceeded its record limit.' }
if ((Payload 16).native_memory.evidence.result.stop_reason -ne 'record_limit' -or -not (Payload 16).truncated) { throw 'Record limit did not freeze and mark incomplete.' }
if ((Payload 19).native_memory.evidence.result.recording -or (Payload 20).native_memory.evidence.result.stop_reason -ne 'deadline') { throw 'Capture deadline did not stop accounting.' }
if ($result.ExitCode -ne 0) { throw 'MCP did not exit cleanly.' }
if (Test-Path -LiteralPath (Join-Path $fixtureRoot 'forbidden.txt')) { throw 'Console path bypass wrote a file.' }
[pscustomobject]@{passed=$true;known_native_bytes=528384;held_proc_bytes=$heldProc[0].outstanding_requested_bytes;released_proc_bytes=$releasedBytes;helper_sha256=(Payload 8).helper_sha256;stdio_exit=$result.ExitCode;evidence=$evidence} | ConvertTo-Json
