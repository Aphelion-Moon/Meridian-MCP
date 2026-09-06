[CmdletBinding()]
param([Parameter(Mandatory)][string]$UpstreamPath)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'spacemandmm-surface.ps1')
$sources = Read-SpacemanSources (Resolve-Path -LiteralPath $UpstreamPath).Path
$observed = @(Get-SpacemanSurface $sources)
$registry = Get-Content (Join-Path $PSScriptRoot '../spacemandmm-capabilities.json') -Raw | ConvertFrom-Json
$evidence = @($registry.capabilities.evidence)
$localWire = Get-Content (Join-Path $PSScriptRoot '../src/spaceman/debugger.rs') -Raw
$baselineErrors = @(Get-SpacemanCoverageErrors $observed $evidence) + @(Get-SpacemanWireErrors $sources.wire $localWire)
if ($baselineErrors.Count) { throw ($baselineErrors -join "`n") }

$checks = 0
# Every discovered capability must have its own usable mapping. Removing any one
# token must fail even if other tokens still map its whole crate or feature group.
foreach ($token in $observed) {
    $without = @($evidence | Where-Object { $_ -cne $token })
    if (-not @(Get-SpacemanCoverageErrors $observed $without).Count) { throw "Accepted missing mapping $token" }
    $checks++
}
foreach ($mutation in @(
    @('language', '        QueryObjectTree;', "        QueryObjectTree;`n        FutureRequest;", 'lsp-request:FutureRequest'),
    @('language', '        Reparse;', "        Reparse;`n        FutureNotification;", 'lsp-notification:FutureNotification'),
    @('language', '                hover_provider:', "                future_provider: Some(true),`n                hover_provider:", 'lsp-provider:future_provider'),
    @('debugger', '        Source;', "        Source;`n        FutureDebuggerRequest;", 'dap-request:FutureDebuggerRequest'),
    @('maps', '    RenderMany,', "    RenderMany,`n    FutureMapCommand,", 'dmm-command:FutureMapCommand'),
    @('wire', '    Pause,', "    Pause,`n    FutureWireVariant,", 'aux-request:FutureWireVariant'),
    @('dmdoc', 'arg == "--dry-run"', 'arg == "--future"', 'dmdoc-option:--future'),
    @('dreamchecker', 'arg == "--parse-only"', 'arg == "--future"', 'dreamchecker-option:--future'),
    @('workspace', '    "crates/dmdoc",', "    `"crates/dmdoc`",`n    `"crates/future-crate`",", 'workspace:future-crate')
)) {
    $changed = $sources.Clone()
    $changed[$mutation[0]] = $changed[$mutation[0]].Replace($mutation[1], $mutation[2])
    $errors = @(Get-SpacemanCoverageErrors @(Get-SpacemanSurface $changed) $evidence)
    if (-not ($errors -like "*$($mutation[3])*")) { throw "Missed added source capability $($mutation[3])" }
    $checks++
}
if (-not @(Get-SpacemanCoverageErrors @($observed | Where-Object { $_ -ne 'lsp-request:HoverRequest' }) $evidence).Count) { throw 'Accepted stale upstream mapping' }
$checks++
foreach ($changedWire in @(
    $localWire.Replace("    Disconnect,`r`n    Configured,", "    Configured,`r`n    Disconnect,").Replace("    Disconnect,`n    Configured,", "    Configured,`n    Disconnect,"),
    $localWire.Replace('pub override_id: u32', 'pub override_id: u64'),
    $localWire.Replace('pub struct VariablesRef(pub i32)', 'pub struct VariablesRef(pub u32)')
)) {
    if (-not @(Get-SpacemanWireErrors $sources.wire $changedWire).Count) { throw 'Accepted changed debugger wire layout' }
    $checks++
}
$broken = $sources.Clone()
$broken.language = $broken.language.Replace('handle_method_call_table! {', 'unrecognized_table! {')
$rejected = $false
try { Get-SpacemanSurface $broken | Out-Null } catch { $rejected = $_.Exception.Message -like '*Expected exactly one*' }
if (-not $rejected) { throw 'Accepted missing source layout' }
$checks++
$lf = @{}
$crlf = @{}
foreach ($key in $sources.Keys) {
    $lf[$key] = $sources[$key].Replace("`r`n", "`n")
    $crlf[$key] = $lf[$key].Replace("`n", "`r`n")
}
if (@(Compare-Object @(Get-SpacemanSurface $lf) @(Get-SpacemanSurface $crlf)).Count) { throw 'LF/CRLF changed inventory' }
$checks++
Write-Output "SpacemanDMM capability checks passed: $checks assertions, including mapping removals, source additions, wire drift, missing layout and LF/CRLF parity."
