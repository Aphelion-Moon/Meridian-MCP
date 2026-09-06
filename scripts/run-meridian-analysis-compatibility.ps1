[CmdletBinding()]
param(
	[Parameter(Mandatory)][string]$MeridianRiftRoot,
	[Parameter(Mandatory)][string]$BinaryPath,
	[Parameter(Mandatory)][string]$HelperManifestPath,
	[Parameter(Mandatory)][string]$EvidencePath
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$mcpRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$riftRoot = (Resolve-Path -LiteralPath $MeridianRiftRoot).Path
$manifest = Get-Content -LiteralPath (Join-Path $mcpRoot 'tests/compatibility/meridian-rift.json') -Raw | ConvertFrom-Json
Import-Module (Join-Path $PSScriptRoot 'MeridianMcpSession.psm1') -Force

function New-Call([int]$Id, [string]$Name, [hashtable]$Arguments) {
	return ConvertTo-McpJsonLine ([ordered]@{ jsonrpc = '2.0'; id = $Id; method = 'tools/call'; params = [ordered]@{ name = $Name; arguments = $Arguments } })
}

$requests = [System.Collections.Generic.List[string]]::new()
$requests.Add((ConvertTo-McpJsonLine ([ordered]@{ jsonrpc = '2.0'; id = 1; method = 'initialize'; params = [ordered]@{ protocolVersion = '2024-11-05'; capabilities = [ordered]@{}; clientInfo = [ordered]@{ name = 'meridian-ubuntu-analysis'; version = '1.0' } } })))
$requests.Add((ConvertTo-McpJsonLine ([ordered]@{ jsonrpc = '2.0'; method = 'notifications/initialized'; params = [ordered]@{} })))
$requests.Add((ConvertTo-McpJsonLine ([ordered]@{ jsonrpc = '2.0'; id = 2; method = 'tools/list'; params = [ordered]@{} })))
$requests.Add((New-Call 3 'dm_server_status' @{}))
$requests.Add((New-Call 4 'dm_parse_environment' @{ dme_path = (Join-Path $riftRoot 'tgstation.dme') }))
$requests.Add((New-Call 5 'dm_check_errors' @{}))
$requests.Add((New-Call 6 'dm_find_implementations' @{ type_path = '/mob/living/carbon/human'; member_name = 'Initialize'; limit = 100 }))
$id = 10
$variableCases = @{}
foreach ($case in $manifest.vars) {
	$variableCases[$id] = $case
	$requests.Add((New-Call $id 'dm_get_var' @{ type_path = $case.type_path; var_name = $case.name }))
	$id++
}
foreach ($relative in $manifest.dmis) {
	$requests.Add((New-Call $id 'dm_dmi_info' @{ dmi_path = (Join-Path $riftRoot $relative) }))
	$id++
}
foreach ($relative in $manifest.maps) {
	$requests.Add((New-Call $id 'dm_map_info' @{ dmm_path = (Join-Path $riftRoot $relative) }))
	$id++
}
$requests.Add((New-Call $id 'dm_list_render_passes' @{}))
$lastId = $id

$environment = @{
	MERIDIAN_MCP_MODE = 'analysis'
	MERIDIAN_MCP_ROOTS = [string]::Join([IO.Path]::PathSeparator, @($mcpRoot, $riftRoot))
	MERIDIAN_MCP_HELPER_MANIFEST = (Resolve-Path -LiteralPath $HelperManifestPath).Path
}
$session = Invoke-McpSession -BinaryPath $BinaryPath -WorkingDirectory $mcpRoot -Environment $environment -Requests $requests.ToArray() -TimeoutMilliseconds 1800000
if ($session.ExitCode -ne 0) { throw "Meridian-MCP exited with $($session.ExitCode)." }
foreach ($responseId in @(1, 2, 3, 4, 5, 6) + (10..$lastId)) {
	$response = Get-McpResponse -Responses $session.Responses -Id $responseId
	if ($responseId -ge 3 -and $response.result.isError -eq $true) {
		throw "Tool request $responseId failed: $($response.result.content[0].text)"
	}
}
$tools = (Get-McpResponse -Responses $session.Responses -Id 2).result.tools.name
foreach ($caseId in $variableCases.Keys) {
	$case = $variableCases[$caseId]
	$body = (Get-McpResponse -Responses $session.Responses -Id $caseId).result.content[0].text | ConvertFrom-Json
	if ($body.name -ne $case.name) { throw "Variable lookup failed for $($case.type_path)/$($case.name)." }
	foreach ($field in @('value_owner', 'declaration_owner', 'constant')) {
		if ($case.PSObject.Properties[$field] -and $body.$field -cne $case.$field) { throw "Unexpected $field for $($case.type_path)/$($case.name)." }
	}
	if ($case.PSObject.Properties['inherited_from'] -and $body.declaration_owner -cne $case.inherited_from) { throw "Wrong variable declaration owner for $($case.name)." }
}
foreach ($forbidden in @('rift_compile', 'dm_debug_launch')) {
	if ($tools -contains $forbidden) { throw "$forbidden must not be advertised by the Ubuntu analysis gate." }
}
$statusText = [string](Get-McpResponse -Responses $session.Responses -Id 3).result.content[0].text
$status = $statusText | ConvertFrom-Json
$ownershipText = [string](Get-McpResponse -Responses $session.Responses -Id 6).result.content[0].text
if ($ownershipText -notmatch '/mob/living/carbon/human') { throw 'Canonical child implementation query returned no human implementation.' }
$rootSources = @($status.containment.effective_roots | ForEach-Object { [string]$_.source } | Sort-Object -Unique)
$evidence = [ordered]@{
	schema_version = 2
	overall = 'passed'
	platform = [ordered]@{ os = [Runtime.InteropServices.RuntimeInformation]::OSDescription; architecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() }
	repositories = [ordered]@{ meridian_mcp = (& git -C $mcpRoot rev-parse HEAD).Trim(); meridian_rift = (& git -C $riftRoot rev-parse HEAD).Trim() }
	spacemandmm_revision = '351ddc0ffb2439876d4565ce5130bb6b027ee605'
	manifest_schema_version = $manifest.schema_version
	effective_root_sources = $rootSources
	proc_ownership_query = [ordered]@{ type_path = '/mob/living/carbon/human'; member_name = 'Initialize'; result = 'passed' }
	variable_ownership_queries = [ordered]@{ cases = $variableCases.Count; result = 'passed' }
	last_request_id = $lastId
}
$evidenceFile = [IO.Path]::GetFullPath($EvidencePath)
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $evidenceFile) | Out-Null
[IO.File]::WriteAllText($evidenceFile, (($evidence | ConvertTo-Json -Depth 8) + [Environment]::NewLine), [Text.UTF8Encoding]::new($false))
