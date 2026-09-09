function Wait-ProcessReadiness {
	[CmdletBinding()]
	param(
		[Parameter(Mandatory)][Diagnostics.Process]$Process,
		[Parameter(Mandatory)][string]$MarkerPath,
		[Parameter(Mandatory)][string]$ExpectedMarker,
		[Parameter(Mandatory)][ValidateRange(1, 900)][int]$TimeoutSeconds
	)

	$stopwatch = [Diagnostics.Stopwatch]::StartNew()
	$samples = [Collections.Generic.List[object]]::new()
	$processExited = $false
	$processExitCode = $null
	$lastCpuMilliseconds = $null
	$lastWorkingSetBytes = $null
	$lastPrivateMemoryBytes = $null
	$lastProgressMilliseconds = 0
	$lastRetainedSampleMilliseconds = -1000
	do {
		$elapsedMilliseconds = [int64]$stopwatch.Elapsed.TotalMilliseconds
		$finalSample = $stopwatch.Elapsed.TotalSeconds -ge $TimeoutSeconds
		$markerExists = Test-Path -LiteralPath $MarkerPath -PathType Leaf
		$cpuMilliseconds = $null
		$workingSetBytes = $null
		$privateMemoryBytes = $null
		$mainWindowPresent = $false
		try {
			$Process.Refresh()
			$processExited = $processExited -or $Process.HasExited
			if ($processExited) {
				$processExitCode = [int32]$Process.ExitCode
			} else {
				$cpuMilliseconds = [int64]$Process.TotalProcessorTime.TotalMilliseconds
				$workingSetBytes = [int64]$Process.WorkingSet64
				$privateMemoryBytes = [int64]$Process.PrivateMemorySize64
				if ($IsWindows) { $mainWindowPresent = $Process.MainWindowHandle -ne [IntPtr]::Zero }
			}
		} catch {
			$processExited = $true
		}

		$hasProcessProgress = ($null -ne $cpuMilliseconds -and $null -ne $lastCpuMilliseconds -and $cpuMilliseconds -gt $lastCpuMilliseconds) -or
			($null -ne $workingSetBytes -and $null -ne $lastWorkingSetBytes -and $workingSetBytes -ne $lastWorkingSetBytes) -or
			($null -ne $privateMemoryBytes -and $null -ne $lastPrivateMemoryBytes -and $privateMemoryBytes -ne $lastPrivateMemoryBytes)
		if ($hasProcessProgress -or $markerExists) { $lastProgressMilliseconds = $elapsedMilliseconds }
		if ($null -ne $cpuMilliseconds) { $lastCpuMilliseconds = $cpuMilliseconds }
		if ($null -ne $workingSetBytes) { $lastWorkingSetBytes = $workingSetBytes }
		if ($null -ne $privateMemoryBytes) { $lastPrivateMemoryBytes = $privateMemoryBytes }

		if ($samples.Count -eq 0 -or $finalSample -or ($elapsedMilliseconds - $lastRetainedSampleMilliseconds) -ge 1000) {
			$samples.Add([pscustomobject]@{
				elapsed_milliseconds = $elapsedMilliseconds
				pid = $Process.Id
				has_exited = $processExited
				total_processor_milliseconds = $cpuMilliseconds
				working_set_bytes = $workingSetBytes
				private_memory_bytes = $privateMemoryBytes
				marker_exists = $markerExists
				main_window_present = $mainWindowPresent
			})
			$lastRetainedSampleMilliseconds = $elapsedMilliseconds
		}

		# Retain one final observation after a delayed poll or sleep. It can
		# establish process progress/exit, but cannot make a late marker ready.
		if ($finalSample) { break }

		if ($markerExists) {
			$marker = Get-Content -Raw -LiteralPath $MarkerPath -ErrorAction SilentlyContinue
			if ($null -ne $marker -and $marker.TrimEnd() -eq $ExpectedMarker -and $stopwatch.Elapsed.TotalSeconds -lt $TimeoutSeconds) {
				$stopwatch.Stop()
				return [pscustomobject]@{
					status = 'ready'
					elapsed_milliseconds = [int64]$stopwatch.Elapsed.TotalMilliseconds
					elapsed_seconds = $stopwatch.Elapsed.TotalSeconds
					samples = @($samples)
					last_progress_milliseconds = $lastProgressMilliseconds
					process_exit_code = $processExitCode
				}
			}
		}

		Start-Sleep -Milliseconds 250
	} while ($true)

	$stopwatch.Stop()
	return [pscustomobject]@{
		status = if ($processExited) { 'process_exited' } else { 'timed_out' }
		elapsed_milliseconds = [int64]$stopwatch.Elapsed.TotalMilliseconds
		elapsed_seconds = $stopwatch.Elapsed.TotalSeconds
		samples = @($samples)
		last_progress_milliseconds = $lastProgressMilliseconds
		process_exit_code = $processExitCode
	}
}

function Get-PrototypeRuntimeClassification {
	[CmdletBinding()]
	param(
		[Parameter(Mandatory)][ValidateSet('control', 'boundary')][string]$RuntimeCase,
		[Parameter(Mandatory)][bool]$CompileSucceeded,
		[Parameter(Mandatory)][bool]$MarkerReady,
		[Parameter(Mandatory)][string]$ReadinessStatus,
		[Parameter(Mandatory)][bool]$HasProcessProgress,
		[Parameter(Mandatory)][bool]$ControlPassed
	)

	if (-not $CompileSucceeded) { return 'compile_failure' }
	if ($MarkerReady) { return 'passed' }
	if ($RuntimeCase -eq 'control') { return 'environment_failure' }
	if ($ReadinessStatus -eq 'timed_out' -and $HasProcessProgress) { return 'inconclusive_timeout' }
	if ($ControlPassed) { return 'boundary_regression' }
	return 'environment_failure'
}

function Get-ByondVersionIdentity {
	[CmdletBinding()]
	param(
		[Parameter(Mandatory)][ValidatePattern('^[0-9]+\.[0-9]+$')][string]$ExpectedVersion,
		[AllowNull()][AllowEmptyString()][string]$ReportedFileVersion
	)

	if ([string]::IsNullOrWhiteSpace($ReportedFileVersion)) {
		return [ordered]@{
			version = $ExpectedVersion
			verification = 'expected_input_only'
		}
	}

	try {
		$parsed = [Version]$ReportedFileVersion
	} catch {
		throw "DreamMaker reported an invalid file version: $ReportedFileVersion"
	}
	$detectedVersion = if ($parsed.Build -ge 0 -and $parsed.Revision -ge 0) {
		"$($parsed.Build).$($parsed.Revision)"
	} else {
		"$($parsed.Major).$($parsed.Minor)"
	}
	if ($detectedVersion -ne $ExpectedVersion) {
		throw "DreamMaker version $detectedVersion does not match expected BYOND $ExpectedVersion."
	}
	return [ordered]@{
		version = $detectedVersion
		verification = 'file_metadata_match'
	}
}

function New-PrototypeDreamDaemonArguments {
	[CmdletBinding()]
	param([Parameter(Mandatory)][string]$DmbPath)

	return @($DmbPath, '0', '-ip', '127.0.0.1', '-trusted', '-log', 'dreamdaemon.world.log', '-close', '-verbose')
}

function ConvertTo-PublicPrototypeFixtureEvidence {
	[CmdletBinding()]
	param([Parameter(Mandatory)][System.Collections.IDictionary]$FixtureMetadata)

	return [ordered]@{
		layout = $FixtureMetadata.layout
		declared_leaf_count = $FixtureMetadata.declared_leaf_count
		declared_parent_count = $FixtureMetadata.declared_parent_count
		declared_type_count = $FixtureMetadata.declared_type_count
		first_path = $FixtureMetadata.first_path
		boundary_path = $FixtureMetadata.boundary_path
		last_path = $FixtureMetadata.last_path
	}
}

Export-ModuleMember -Function Wait-ProcessReadiness, Get-PrototypeRuntimeClassification, Get-ByondVersionIdentity, New-PrototypeDreamDaemonArguments, ConvertTo-PublicPrototypeFixtureEvidence
