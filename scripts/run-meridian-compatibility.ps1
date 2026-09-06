[CmdletBinding()]
param(
	[Parameter(Mandatory)][string]$BinaryPath,
	[Parameter(Mandatory)][string]$MeridianRiftRoot,
	[Parameter(Mandatory)][string]$DreamMakerPath,
	[Parameter(Mandatory)][string]$EvidencePath,
	[string]$MeridianMcpSha,
	[string]$MeridianRiftSha
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$mcpRoot = Split-Path -Parent $scriptRoot
Import-Module (Join-Path $scriptRoot 'MeridianMcpSession.psm1') -Force

$maximumCapturedCharacters = 524288
$evidence = [ordered]@{
	schema_version = 1
	overall = 'failed'
	first_failing_stage = $null
	started_at_utc = [DateTime]::UtcNow.ToString('o')
	finished_at_utc = $null
	repositories = [ordered]@{}
	platform = [ordered]@{}
	versions = [ordered]@{}
	configuration = [ordered]@{
		capability_mode = 'development'
		rift_build_ceiling = 'network'
	}
	manifest = [ordered]@{}
	assertions = @()
	timings_ms = [ordered]@{}
	builds = [ordered]@{}
	negative_sessions = @()
	warnings = @()
}
$temporaryFiles = [System.Collections.Generic.List[string]]::new()
$systemTemporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$stateDirectory = Join-Path $systemTemporaryRoot ('.meridian-compatibility-state-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stateDirectory | Out-Null
$stateDirectory = (Resolve-Path -LiteralPath $stateDirectory).Path
$relativeStateDirectory = [IO.Path]::GetRelativePath($systemTemporaryRoot, $stateDirectory)
if ($relativeStateDirectory -eq '..' -or $relativeStateDirectory.StartsWith('..' + [IO.Path]::DirectorySeparatorChar)) {
	throw 'Temporary state directory resolved outside the operating-system temporary directory.'
}
$temporaryFiles.Add($stateDirectory)

function Limit-CapturedText {
	param(
		[AllowNull()][string]$Text,
		[int]$MaximumCharacters = $maximumCapturedCharacters
	)
	if ($null -eq $Text) {
		return ''
	}
	if ($Text.Length -le $MaximumCharacters) {
		return $Text
	}
	return $Text.Substring($Text.Length - $MaximumCharacters)
}

function Get-RepositorySha {
	param([Parameter(Mandatory)][string]$Root, [AllowEmptyString()][string]$Provided)
	if (-not [string]::IsNullOrWhiteSpace($Provided)) {
		return $Provided
	}
	$sha = & git -C $Root rev-parse HEAD 2>$null
	if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($sha)) {
		return $null
	}
	return ([string]$sha).Trim()
}

function Resolve-ContainedFile {
	param(
		[Parameter(Mandatory)][string]$Root,
		[Parameter(Mandatory)][string]$RelativePath
	)
	$resolvedRoot = (Resolve-Path -LiteralPath $Root).Path.TrimEnd('\', '/')
	$candidate = [System.IO.Path]::GetFullPath((Join-Path $resolvedRoot $RelativePath))
	if (-not [string]::Equals((Split-Path -Parent $candidate), $resolvedRoot, [StringComparison]::OrdinalIgnoreCase)) {
		throw "Refusing non-root artifact path: $candidate"
	}
	return $candidate
}

function Remove-CompilerArtifacts {
	param([Parameter(Mandatory)][string]$Root)
	foreach ($name in @('tgstation.dmb', 'tgstation.rsc')) {
		$path = Resolve-ContainedFile -Root $Root -RelativePath $name
		if (Test-Path -LiteralPath $path -PathType Leaf) {
			Remove-Item -LiteralPath $path -Force
		}
	}
}

function Invoke-HumanBuild {
	param(
		[Parameter(Mandatory)][string]$Root,
		[Parameter(Mandatory)][string]$DreamMakerPath
	)
	$buildPath = Resolve-ContainedFile -Root $Root -RelativePath 'BUILD.cmd'
	if (-not (Test-Path -LiteralPath $buildPath -PathType Leaf)) {
		throw "Human build entry point is missing: $buildPath"
	}
	$systemDirectory = [Environment]::GetFolderPath([Environment+SpecialFolder]::System)
	$commandProcessor = Join-Path $systemDirectory 'cmd.exe'
	$startInfo = [Diagnostics.ProcessStartInfo]::new()
	$startInfo.FileName = $commandProcessor
	$startInfo.Arguments = '/D /S /C "call BUILD.cmd"'
	$startInfo.WorkingDirectory = $Root
	$startInfo.UseShellExecute = $false
	$startInfo.CreateNoWindow = $true
	$startInfo.RedirectStandardOutput = $true
	$startInfo.RedirectStandardError = $true
	$startInfo.Environment['DM_EXE'] = $DreamMakerPath
	$process = [Diagnostics.Process]::new()
	$process.StartInfo = $startInfo
	$startedAt = [DateTime]::UtcNow
	if (-not $process.Start()) {
		throw 'Failed to start BUILD.cmd.'
	}
	try {
		$stdoutTask = $process.StandardOutput.ReadToEndAsync()
		$stderrTask = $process.StandardError.ReadToEndAsync()
		if (-not $process.WaitForExit(1800000)) {
			try {
				$process.Kill($true)
			} catch {
				$process.Kill()
			}
			$process.WaitForExit()
			return [ordered]@{
				success = $false
				exit_code = $null
				timed_out = $true
				duration_ms = [int64]([DateTime]::UtcNow - $startedAt).TotalMilliseconds
				stdout = Limit-CapturedText $stdoutTask.GetAwaiter().GetResult()
				stderr = Limit-CapturedText $stderrTask.GetAwaiter().GetResult()
			}
		}
		return [ordered]@{
			success = $process.ExitCode -eq 0
			exit_code = $process.ExitCode
			timed_out = $false
			duration_ms = [int64]([DateTime]::UtcNow - $startedAt).TotalMilliseconds
			stdout = Limit-CapturedText $stdoutTask.GetAwaiter().GetResult()
			stderr = Limit-CapturedText $stderrTask.GetAwaiter().GetResult()
		}
	} finally {
		if (-not $process.HasExited) {
			try { $process.Kill($true) } catch { $process.Kill() }
			$process.WaitForExit()
		}
		$process.Dispose()
	}
}

function New-ToolCall {
	param(
		[Parameter(Mandatory)][int]$Id,
		[Parameter(Mandatory)][string]$Name,
		[Parameter(Mandatory)][System.Collections.IDictionary]$Arguments
	)
	return ConvertTo-McpJsonLine ([ordered]@{
		jsonrpc = '2.0'
		id = $Id
		method = 'tools/call'
		params = [ordered]@{ name = $Name; arguments = $Arguments }
	})
}

function Get-ToolPayload {
	param(
		[Parameter(Mandatory)][object[]]$Responses,
		[Parameter(Mandatory)][int]$Id,
		[Parameter(Mandatory)][string]$Stage,
		[switch]$AllowToolError
	)
	$response = Get-McpResponse -Responses $Responses -Id $Id
	if ($response.result.isError -eq $true -and -not $AllowToolError) {
		throw "$Stage returned a tool error: $($response.result.content[0].text)"
	}
	$text = [string]$response.result.content[0].text
	try {
		$payload = $text | ConvertFrom-Json
	} catch {
		if ($AllowToolError) {
			return [pscustomobject]@{ raw_text = $text; is_error = $response.result.isError -eq $true }
		}
		throw "$Stage returned non-JSON content: $text"
	}
	$payload | Add-Member -NotePropertyName _tool_error -NotePropertyValue ($response.result.isError -eq $true) -Force
	return $payload
}

function Assert-True {
	param([Parameter(Mandatory)][bool]$Condition, [Parameter(Mandatory)][string]$Message)
	if (-not $Condition) {
		throw $Message
	}
}

function Test-PathSuffix {
	param([AllowNull()][string]$Actual, [Parameter(Mandatory)][string]$Suffix)
	if ([string]::IsNullOrWhiteSpace($Actual)) {
		return $false
	}
	$normalizedActual = $Actual.Replace('\', '/').ToLowerInvariant()
	$normalizedSuffix = $Suffix.Replace('\', '/').ToLowerInvariant()
	return $normalizedActual.EndsWith($normalizedSuffix, [StringComparison]::Ordinal)
}

function Get-LocationPath {
	param([Parameter(Mandatory)][string]$Location)
	return ($Location -replace ':\d+(?::\d+)?$', '')
}

function Add-AssertionEvidence {
	param([string]$Tool, [string]$Case, [int]$Id, [int64]$DurationMilliseconds)
	$evidence.assertions += [ordered]@{
		tool = $Tool
		case = $Case
		request_id = $Id
		passed = $true
		duration_ms = $DurationMilliseconds
	}
}

function Invoke-NegativeSession {
	param(
		[Parameter(Mandatory)][string]$Name,
		[Parameter(Mandatory)][string]$Mode,
		[Parameter(Mandatory)][string]$Ceiling,
		[Parameter(Mandatory)][string[]]$Requests,
		[hashtable]$ExtraEnvironment = @{}
	)
	$sessionEnvironment = @{
		MERIDIAN_MCP_MODE = $Mode
		MERIDIAN_MCP_ROOTS = [string]::Join([IO.Path]::PathSeparator, @($mcpRoot, $MeridianRiftRoot))
		MERIDIAN_MCP_COMPILERS = $DreamMakerPath
		MERIDIAN_MCP_RIFT_BUILD = $Ceiling
	}
	if ($Mode -eq 'development') {
		$sessionEnvironment.MERIDIAN_MCP_STATE_DIR = $stateDirectory
	}
	foreach ($entry in $ExtraEnvironment.GetEnumerator()) {
		$sessionEnvironment[$entry.Key] = $entry.Value
	}
	$session = Invoke-McpSession -BinaryPath $BinaryPath -WorkingDirectory $mcpRoot -Environment $sessionEnvironment -Requests $Requests -TimeoutMilliseconds 180000
	Assert-True ($session.ExitCode -eq 0) "$Name MCP session exited with $($session.ExitCode)."
	return $session
}

function Assert-NoSensitiveEvidenceKeys {
	param([AllowNull()][Parameter(Mandatory)]$Value, [string]$Path = '$')
	if ($Value -is [System.Collections.IDictionary]) {
		foreach ($key in $Value.Keys) {
			if ([string]$key -match '(?i)(token|secret|password|authorization|cookie)') {
				throw "Evidence contains a forbidden key at $Path.$key"
			}
			Assert-NoSensitiveEvidenceKeys -Value $Value[$key] -Path "$Path.$key"
		}
		return
	}
	if ($Value -is [System.Management.Automation.PSCustomObject]) {
		foreach ($property in $Value.PSObject.Properties) {
			if ($property.Name -match '(?i)(token|secret|password|authorization|cookie)') {
				throw "Evidence contains a forbidden key at $Path.$($property.Name)"
			}
			Assert-NoSensitiveEvidenceKeys -Value $property.Value -Path "$Path.$($property.Name)"
		}
		return
	}
	if ($Value -is [System.Collections.IEnumerable] -and $Value -isnot [string]) {
		$index = 0
		foreach ($item in $Value) {
			Assert-NoSensitiveEvidenceKeys -Value $item -Path "$Path[$index]"
			$index++
		}
	}
}

try {
	$BinaryPath = (Resolve-Path -LiteralPath $BinaryPath).Path
	$MeridianRiftRoot = (Resolve-Path -LiteralPath $MeridianRiftRoot).Path
	$DreamMakerPath = (Resolve-Path -LiteralPath $DreamMakerPath).Path
	$dmePath = Resolve-ContainedFile -Root $MeridianRiftRoot -RelativePath 'tgstation.dme'
	$manifestPath = Join-Path $mcpRoot 'tests\compatibility\meridian-rift.json'
	$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
	$evidence.manifest = [ordered]@{ schema_version = $manifest.schema_version; path = 'tests/compatibility/meridian-rift.json' }
	$evidence.repositories = [ordered]@{
		meridian_mcp = [ordered]@{ sha = Get-RepositorySha -Root $mcpRoot -Provided $MeridianMcpSha }
		meridian_rift = [ordered]@{ sha = Get-RepositorySha -Root $MeridianRiftRoot -Provided $MeridianRiftSha }
	}
	$evidence.platform = [ordered]@{
		os = [Environment]::OSVersion.VersionString
		architecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
		runner_image = $env:ImageOS
	}
	$dependencies = Get-Content -LiteralPath (Join-Path $MeridianRiftRoot 'dependencies.sh')
	$major = ($dependencies | Select-String '^export BYOND_MAJOR=([0-9]+)$').Matches.Groups[1].Value
	$minor = ($dependencies | Select-String '^export BYOND_MINOR=([0-9]+)$').Matches.Groups[1].Value
	Assert-True (-not [string]::IsNullOrWhiteSpace($major) -and -not [string]::IsNullOrWhiteSpace($minor)) 'dependencies.sh does not contain literal BYOND version pins.'
	$evidence.versions = [ordered]@{
		byond = "$major.$minor"
		powershell = $PSVersionTable.PSVersion.ToString()
		mcp_binary = [IO.Path]::GetFileName($BinaryPath)
	}

	$requests = [System.Collections.Generic.List[string]]::new()
	$requests.Add((ConvertTo-McpJsonLine ([ordered]@{
		jsonrpc = '2.0'; id = 1; method = 'initialize'
		params = [ordered]@{ protocolVersion = '2024-11-05'; capabilities = [ordered]@{}; clientInfo = [ordered]@{ name = 'meridian-compatibility'; version = '1.0' } }
	})))
	$requests.Add((ConvertTo-McpJsonLine ([ordered]@{ jsonrpc = '2.0'; method = 'notifications/initialized'; params = [ordered]@{} })))
	$requests.Add((ConvertTo-McpJsonLine ([ordered]@{ jsonrpc = '2.0'; id = 2; method = 'tools/list'; params = [ordered]@{} })))
	$requests.Add((New-ToolCall -Id 3 -Name 'dm_parse_environment' -Arguments ([ordered]@{ dme_path = $dmePath })))

	$cases = [System.Collections.Generic.List[object]]::new()
	$nextId = 10
	foreach ($case in $manifest.types) {
		$cases.Add([pscustomobject]@{ id = $nextId; tool = 'dm_get_type'; case = $case })
		$requests.Add((New-ToolCall -Id $nextId -Name 'dm_get_type' -Arguments ([ordered]@{ type_path = $case.path })))
		$nextId++
	}
	foreach ($case in $manifest.procs) {
		$cases.Add([pscustomobject]@{ id = $nextId; tool = 'dm_get_proc'; case = $case })
		$requests.Add((New-ToolCall -Id $nextId -Name 'dm_get_proc' -Arguments ([ordered]@{ type_path = $case.type_path; proc_name = $case.name })))
		$nextId++
	}
	foreach ($case in $manifest.vars) {
		$cases.Add([pscustomobject]@{ id = $nextId; tool = 'dm_get_var'; case = $case })
		$requests.Add((New-ToolCall -Id $nextId -Name 'dm_get_var' -Arguments ([ordered]@{ type_path = $case.type_path; var_name = $case.name })))
		$nextId++
	}
	foreach ($case in $manifest.type_lists) {
		$cases.Add([pscustomobject]@{ id = $nextId; tool = 'dm_list_types'; case = $case })
		$requests.Add((New-ToolCall -Id $nextId -Name 'dm_list_types' -Arguments ([ordered]@{ prefix = $case.prefix })))
		$nextId++
	}
	foreach ($case in $manifest.symbol_searches) {
		$cases.Add([pscustomobject]@{ id = $nextId; tool = 'dm_search_symbols'; case = $case })
		$requests.Add((New-ToolCall -Id $nextId -Name 'dm_search_symbols' -Arguments ([ordered]@{ query = $case.query; kind = $case.kind; limit = 50 })))
		$nextId++
	}
	foreach ($case in $manifest.context_searches) {
		$firstId = $nextId
		$secondId = $nextId + 1
		$cases.Add([pscustomobject]@{ id = $firstId; repeat_id = $secondId; tool = 'dm_search_context'; case = $case })
		$arguments = [ordered]@{ query = $case.query; limit = $case.top; include_source = $false }
		$requests.Add((New-ToolCall -Id $firstId -Name 'dm_search_context' -Arguments $arguments))
		$requests.Add((New-ToolCall -Id $secondId -Name 'dm_search_context' -Arguments $arguments))
		$nextId += 2
	}
	foreach ($case in $manifest.definitions) {
		$arguments = [ordered]@{ type_path = $case.type_path }
		if ($case.PSObject.Properties['member']) { $arguments.member_name = $case.member }
		$cases.Add([pscustomobject]@{ id = $nextId; tool = 'dm_get_definition'; case = $case })
		$requests.Add((New-ToolCall -Id $nextId -Name 'dm_get_definition' -Arguments $arguments))
		$nextId++
	}
	$lastAnalysisId = $nextId - 1
	$requests.Add((New-ToolCall -Id 1000 -Name 'dm_compile' -Arguments ([ordered]@{
		dme_path = $dmePath; compiler_path = $DreamMakerPath; working_directory = $MeridianRiftRoot
		timeout_ms = 1800000; idle_timeout_ms = 900000; capture_network = $true
	})))
	$requests.Add((New-ToolCall -Id 1001 -Name 'rift_compile' -Arguments ([ordered]@{
		network_mode = 'allow'; timeout_ms = 1800000; idle_timeout_ms = 900000; capture_network = $true; force_rebuild = $true
	})))
	$requests.Add((New-ToolCall -Id 1002 -Name 'rift_compile' -Arguments ([ordered]@{
		network_mode = 'offline'; timeout_ms = 1800000; idle_timeout_ms = 900000; capture_network = $true; force_rebuild = $true
	})))

	$callbackState = [ordered]@{ human_build = $null }
	$afterResponse = {
		param($request, $response)
		$idProperty = $request.PSObject.Properties['id']
		if ($null -eq $idProperty) { return }
		switch ([int]$idProperty.Value) {
			$lastAnalysisId { Remove-CompilerArtifacts -Root $MeridianRiftRoot }
			1000 { Remove-CompilerArtifacts -Root $MeridianRiftRoot }
			1001 {
				$callbackState.human_build = Invoke-HumanBuild -Root $MeridianRiftRoot -DreamMakerPath $DreamMakerPath
				$evidence.builds['human_warm'] = $callbackState.human_build
				if (-not $callbackState.human_build.success) {
					$stdout = Limit-CapturedText -Text $callbackState.human_build.stdout -MaximumCharacters 32768
					$stderr = Limit-CapturedText -Text $callbackState.human_build.stderr -MaximumCharacters 32768
					Write-Host "Warm BUILD.cmd stdout:`n$stdout"
					Write-Host "Warm BUILD.cmd stderr:`n$stderr"
					if ($callbackState.human_build.timed_out) {
						throw 'The warm human BUILD.cmd gate timed out.'
					}
					throw "The warm human BUILD.cmd gate failed with exit code $($callbackState.human_build.exit_code)."
				}
				Remove-CompilerArtifacts -Root $MeridianRiftRoot
			}
		}
	}

	$sessionEnvironment = @{
		MERIDIAN_MCP_MODE = 'development'
		MERIDIAN_MCP_ROOTS = [string]::Join([IO.Path]::PathSeparator, @($mcpRoot, $MeridianRiftRoot))
		MERIDIAN_MCP_COMPILERS = $DreamMakerPath
		MERIDIAN_MCP_RIFT_BUILD = 'network'
		MERIDIAN_MCP_STATE_DIR = $stateDirectory
	}
	$session = Invoke-McpSession -BinaryPath $BinaryPath -WorkingDirectory $mcpRoot -Environment $sessionEnvironment -Requests $requests.ToArray() -TimeoutMilliseconds 1800000 -AfterResponse $afterResponse
	Assert-True ($session.ExitCode -eq 0) "Compatibility MCP session exited with $($session.ExitCode)."
	$toolsResponse = Get-McpResponse -Responses $session.Responses -Id 2
	$toolNames = @($toolsResponse.result.tools | ForEach-Object { $_.name })
	Assert-True ($toolNames -contains 'rift_compile') 'rift_compile was not advertised under the network development ceiling.'
	$riftTool = @($toolsResponse.result.tools | Where-Object { $_.name -eq 'rift_compile' })[0]
	$riftProperties = @($riftTool.inputSchema.properties.PSObject.Properties.Name | Sort-Object)
	$expectedRiftProperties = @('capture_network', 'fixture_manifest_path', 'force_rebuild', 'idle_timeout_ms', 'network_mode', 'timeout_ms')
	Assert-True ([string]::Join(',', $riftProperties) -eq [string]::Join(',', $expectedRiftProperties)) 'rift_compile advertised an unexpected schema.'

	$parsePayload = Get-ToolPayload -Responses $session.Responses -Id 3 -Stage 'dm_parse_environment'
	Assert-True ($parsePayload.success -eq $true -and $parsePayload.total_types -gt 0 -and $parsePayload.indexed_symbols -gt 0) 'Full-corpus parse returned incomplete metrics.'
	$evidence.timings_ms.parse = [int64]$session.ResponseTimingsMilliseconds['3']
	$evidence.manifest.state_generation = $parsePayload.state_generation
	$evidence.manifest.total_types = $parsePayload.total_types
	$evidence.manifest.indexed_symbols = $parsePayload.indexed_symbols

	foreach ($record in $cases) {
		$payload = Get-ToolPayload -Responses $session.Responses -Id $record.id -Stage $record.tool
		$case = $record.case
		switch ($record.tool) {
			'dm_get_type' {
				Assert-True ($payload.path -eq $case.path) "dm_get_type returned $($payload.path), expected $($case.path)."
				if ($case.PSObject.Properties['parent']) { Assert-True ($payload.parent -eq $case.parent) "Unexpected parent for $($case.path)." }
				if ($case.PSObject.Properties['file_suffix']) { Assert-True (Test-PathSuffix (Get-LocationPath $payload.location) $case.file_suffix) "Unexpected declaration file for $($case.path)." }
			}
			'dm_get_proc' {
				Assert-True ($payload.name -eq $case.name -and @($payload.overrides).Count -gt 0) "Proc lookup failed for $($case.type_path)/$($case.name)."
				if ($case.PSObject.Properties['file_suffix']) { Assert-True (Test-PathSuffix (Get-LocationPath $payload.overrides[0].location) $case.file_suffix) "Unexpected proc file for $($case.name)." }
				if ($case.PSObject.Properties['inherited_from']) { Assert-True ($payload.declared -eq $false) "Expected inherited proc $($case.name)." }
			}
			'dm_get_var' {
				Assert-True ($payload.name -eq $case.name) "Var lookup failed for $($case.type_path)/$($case.name)."
				if ($case.PSObject.Properties['file_suffix']) { Assert-True (Test-PathSuffix (Get-LocationPath $payload.location) $case.file_suffix) "Unexpected var file for $($case.name)." }
				if ($case.PSObject.Properties['inherited_from']) { Assert-True ($payload.declaration_owner -ceq $case.inherited_from) "Expected inherited var declaration $($case.name)." }
				foreach ($field in @('value_owner', 'declaration_owner', 'constant')) {
					if ($case.PSObject.Properties[$field]) { Assert-True ($payload.$field -ceq $case.$field) "Unexpected $field for $($case.type_path)/$($case.name)." }
				}
			}
			'dm_list_types' {
				$paths = @($payload.types | ForEach-Object { $_.path })
				foreach ($expected in $case.contains) { Assert-True ($paths -contains $expected) "Type list omitted $expected." }
			}
			'dm_search_symbols' {
				$names = @($payload.results | ForEach-Object { if ($_.PSObject.Properties['name']) { $_.name } else { $_.path } })
				Assert-True ($names -contains $case.contains_name) "Symbol search omitted $($case.contains_name)."
			}
			'dm_search_context' {
				$symbols = @($payload.results | ForEach-Object { $_.symbol })
				Assert-True ($symbols -contains $case.contains_symbol) "Context search omitted $($case.contains_symbol)."
				$repeat = Get-ToolPayload -Responses $session.Responses -Id $record.repeat_id -Stage 'dm_search_context repeat'
				$repeatSymbols = @($repeat.results | ForEach-Object { $_.symbol })
				Assert-True ([string]::Join("`n", $symbols) -eq [string]::Join("`n", $repeatSymbols)) "Context search ordering was not deterministic for $($case.query)."
			}
			'dm_get_definition' {
				Assert-True ($payload.kind -eq $case.kind) "Definition kind mismatch for $($case.type_path)."
				if ($case.PSObject.Properties['file_suffix']) { Assert-True (Test-PathSuffix $payload.file $case.file_suffix) "Unexpected definition file for $($case.type_path)." }
				if ($case.PSObject.Properties['defined_in']) { Assert-True ($payload.defined_in -eq $case.defined_in) "Unexpected inherited definition owner for $($case.member)." }
				Assert-True ($payload.line -gt 0 -and $payload.column -gt 0) 'Definition did not include a valid source span.'
			}
		}
		Add-AssertionEvidence -Tool $record.tool -Case (($case | ConvertTo-Json -Compress -Depth 5)) -Id $record.id -DurationMilliseconds ([int64]$session.ResponseTimingsMilliseconds[[string]$record.id])
	}

	$directBuild = Get-ToolPayload -Responses $session.Responses -Id 1000 -Stage 'dm_compile'
	Assert-True ($directBuild.success -eq $true) 'dm_compile did not report success.'
	Assert-True ($directBuild.artifact_after.exists -eq $true) 'dm_compile did not report a produced DMB artifact.'
	$networkBuild = Get-ToolPayload -Responses $session.Responses -Id 1001 -Stage 'rift_compile allow'
	Assert-True ($networkBuild.success -eq $true -and $networkBuild.evidence -eq 'fresh_artifacts') 'Network-enabled rift_compile did not produce fresh artifacts.'
	$offlineBuild = Get-ToolPayload -Responses $session.Responses -Id 1002 -Stage 'rift_compile offline'
	Assert-True ($offlineBuild.success -eq $true -and $offlineBuild.evidence -eq 'fresh_artifacts') 'Offline rift_compile did not produce fresh artifacts.'
	$evidence.builds = [ordered]@{
		direct = $directBuild
		rift_network = $networkBuild
		human_warm = $callbackState.human_build
		rift_offline = $offlineBuild
	}
	$evidence.timings_ms.direct_build = [int64]$session.ResponseTimingsMilliseconds['1000']
	$evidence.timings_ms.rift_network = [int64]$session.ResponseTimingsMilliseconds['1001']
	$evidence.timings_ms.rift_offline = [int64]$session.ResponseTimingsMilliseconds['1002']
	if ($session.Stderr) { $evidence.warnings += (Limit-CapturedText $session.Stderr) }

	$baseInitialize = ConvertTo-McpJsonLine ([ordered]@{ jsonrpc = '2.0'; id = 1; method = 'initialize'; params = [ordered]@{ protocolVersion = '2024-11-05'; capabilities = [ordered]@{}; clientInfo = [ordered]@{ name = 'meridian-negative'; version = '1.0' } } })
	$baseInitialized = ConvertTo-McpJsonLine ([ordered]@{ jsonrpc = '2.0'; method = 'notifications/initialized'; params = [ordered]@{} })
	$listRequest = ConvertTo-McpJsonLine ([ordered]@{ jsonrpc = '2.0'; id = 2; method = 'tools/list'; params = [ordered]@{} })
	foreach ($visibilityCase in @(
		@{ name = 'disabled_visibility'; mode = 'development'; ceiling = 'disabled' },
		@{ name = 'analysis_visibility'; mode = 'analysis'; ceiling = 'network' }
	)) {
		$negative = Invoke-NegativeSession -Name $visibilityCase.name -Mode $visibilityCase.mode -Ceiling $visibilityCase.ceiling -Requests @($baseInitialize, $baseInitialized, $listRequest)
		$names = @((Get-McpResponse -Responses $negative.Responses -Id 2).result.tools | ForEach-Object { $_.name })
		Assert-True ($names -notcontains 'rift_compile') "$($visibilityCase.name) advertised rift_compile."
		$evidence.negative_sessions += [ordered]@{ name = $visibilityCase.name; passed = $true }
	}

	$emptyCache = Resolve-ContainedFile -Root $MeridianRiftRoot -RelativePath '.meridian-mcp-empty-cache'
	New-Item -ItemType Directory -Path $emptyCache -Force | Out-Null
	$temporaryFiles.Add($emptyCache)
	$denyRequests = @(
		$baseInitialize,
		$baseInitialized,
		(New-ToolCall -Id 3 -Name 'dm_parse_environment' -Arguments ([ordered]@{ dme_path = $dmePath })),
		(New-ToolCall -Id 4 -Name 'rift_compile' -Arguments ([ordered]@{ network_mode = 'allow' })),
		(New-ToolCall -Id 5 -Name 'rift_compile' -Arguments ([ordered]@{ network_mode = 'offline'; force_rebuild = $false }))
	)
	$deny = Invoke-NegativeSession -Name 'offline_policy_cases' -Mode 'development' -Ceiling 'offline' -Requests $denyRequests -ExtraEnvironment @{ TG_BOOTSTRAP_CACHE = $emptyCache }
	$denyPayload = Get-ToolPayload -Responses $deny.Responses -Id 4 -Stage 'offline ceiling denial' -AllowToolError
	Assert-True ($denyPayload._tool_error -eq $true -and $denyPayload.code -eq 'network_mode_denied') 'Offline startup ceiling did not return network_mode_denied.'
	$evidence.negative_sessions += [ordered]@{ name = 'offline_ceiling_denies_allow'; passed = $true; code = $denyPayload.code }
	$coldCachePayload = Get-ToolPayload -Responses $deny.Responses -Id 5 -Stage 'cold offline cache' -AllowToolError
	Assert-True ($coldCachePayload._tool_error -eq $true -and $coldCachePayload.code -eq 'offline_preflight_failed') 'A deliberately empty offline cache did not fail preflight.'
	Assert-True ($coldCachePayload.artifact_before.dmb.sha256 -eq $coldCachePayload.artifact_after.dmb.sha256 -and $coldCachePayload.artifact_before.rsc.sha256 -eq $coldCachePayload.artifact_after.rsc.sha256) 'Cold-cache preflight changed compiler artifacts.'
	$evidence.negative_sessions += [ordered]@{ name = 'cold_offline_cache'; passed = $true; code = $coldCachePayload.code }

	$badDmePath = Resolve-ContainedFile -Root $MeridianRiftRoot -RelativePath '.meridian-mcp-invalid.dme'
	[IO.File]::WriteAllText($badDmePath, '#include "missing-meridian-mcp-file.dm"')
	$temporaryFiles.Add($badDmePath)
	$preserveRequests = @(
		$baseInitialize,
		$baseInitialized,
		(New-ToolCall -Id 3 -Name 'dm_parse_environment' -Arguments ([ordered]@{ dme_path = $dmePath })),
		(New-ToolCall -Id 4 -Name 'dm_parse_environment' -Arguments ([ordered]@{ dme_path = $badDmePath })),
		(New-ToolCall -Id 5 -Name 'dm_get_type' -Arguments ([ordered]@{ type_path = '/datum/controller/subsystem' }))
	)
	$preserve = Invoke-NegativeSession -Name 'failed_reparse_preserves_state' -Mode 'development' -Ceiling 'disabled' -Requests $preserveRequests
	$goodParse = Get-ToolPayload -Responses $preserve.Responses -Id 3 -Stage 'initial parse'
	$badParse = Get-ToolPayload -Responses $preserve.Responses -Id 4 -Stage 'failed reparse' -AllowToolError
	$preservedType = Get-ToolPayload -Responses $preserve.Responses -Id 5 -Stage 'lookup after failed reparse'
	Assert-True ($badParse._tool_error -eq $true -and $badParse.details.state_preserved -eq $true -and $badParse.details.state_generation -eq $goodParse.state_generation) 'Failed reparse did not preserve the active generation.'
	Assert-True ($preservedType.path -eq '/datum/controller/subsystem') 'Lookup failed after a rejected reparse.'
	$evidence.negative_sessions += [ordered]@{ name = 'failed_reparse_preserves_state'; passed = $true; state_generation = $goodParse.state_generation }

	$evidence.overall = 'passed'
} catch {
	$evidence.first_failing_stage = $_.Exception.Message
	throw
} finally {
	foreach ($temporaryFile in $temporaryFiles) {
		if (Test-Path -LiteralPath $temporaryFile -PathType Leaf) {
			Remove-Item -LiteralPath $temporaryFile -Force
		} elseif (Test-Path -LiteralPath $temporaryFile -PathType Container) {
			Remove-Item -LiteralPath $temporaryFile -Recurse -Force
		}
	}
	$evidence.finished_at_utc = [DateTime]::UtcNow.ToString('o')
	$evidenceDirectory = Split-Path -Parent ([IO.Path]::GetFullPath($EvidencePath))
	if (-not (Test-Path -LiteralPath $evidenceDirectory -PathType Container)) {
		New-Item -ItemType Directory -Path $evidenceDirectory -Force | Out-Null
	}
	Assert-NoSensitiveEvidenceKeys -Value $evidence
	$evidence | ConvertTo-Json -Depth 30 | Set-Content -LiteralPath $EvidencePath -Encoding utf8
}
