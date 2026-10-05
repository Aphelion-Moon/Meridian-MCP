$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$scriptPath = Join-Path $PSScriptRoot 'run-meridian-compatibility.ps1'
$scriptSource = Get-Content -Raw -LiteralPath $scriptPath
$tokens = $null
$parseErrors = $null
$syntaxTree = [System.Management.Automation.Language.Parser]::ParseFile(
	$scriptPath,
	[ref]$tokens,
	[ref]$parseErrors
)
if ($parseErrors.Count -ne 0) {
	throw "Compatibility harness has PowerShell parse errors: $($parseErrors[0].Message)"
}
$validator = $syntaxTree.Find({
	param($node)
	return $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq 'Assert-NoSensitiveEvidenceKeys'
}, $true)
if ($null -eq $validator) {
	throw 'Compatibility harness does not define Assert-NoSensitiveEvidenceKeys.'
}
Invoke-Expression $validator.Extent.Text
foreach ($name in @('Limit-CapturedText', 'Get-RiftFailureEvidence')) {
	$function = $syntaxTree.Find({
		param($node)
		return $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq $name
	}, $true)
	if ($null -eq $function) { throw "Compatibility harness does not define $name." }
	Invoke-Expression $function.Extent.Text
}

try {
	Assert-NoSensitiveEvidenceKeys -Value ([ordered]@{
		artifact = [ordered]@{ sha256 = $null; modified_unix_ms = $null }
		warnings = @($null)
	})
} catch {
	throw "Legitimate null evidence was rejected: $($_.Exception.Message)"
}

$forbiddenKeyRejected = $false
try {
	Assert-NoSensitiveEvidenceKeys -Value ([ordered]@{ nested = [ordered]@{ token = $null } })
} catch {
	if ($_.Exception.Message -match 'forbidden key') {
		$forbiddenKeyRejected = $true
	} else {
		throw
	}
}
if (-not $forbiddenKeyRejected) {
	throw 'A forbidden evidence key was accepted.'
}

if ($scriptSource -notmatch [regex]::Escape('$badParse.details.state_preserved') -or $scriptSource -notmatch [regex]::Escape('$badParse.details.state_generation')) {
	throw 'Failed-reparse validation does not read the structured error details object.'
}

$temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$fixtureRoot = Join-Path $temporaryRoot ('meridian-evidence-' + [Guid]::NewGuid().ToString('N'))
$runId = '20261005T081705Z-c3a2f28d'
$runDirectory = Join-Path $fixtureRoot "data/rift-runs/$runId"
New-Item -ItemType Directory -Path $runDirectory -Force | Out-Null
try {
	$summaryPath = Join-Path $runDirectory 'summary.json'
	$summary = [ordered]@{
		schema_version = 1; run_id = $runId; command = 'compile'; status = 'failed'; exit_code = 3
		failures = @([ordered]@{ code = 'probe_failed'; stage = 'preflight'; message = 'fixture failure' })
		environment = [ordered]@{ ignored = 'not diagnostic evidence' }
	}
	$summary | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $summaryPath -Encoding utf8
	$failure = Get-RiftFailureEvidence -Root $fixtureRoot -RunId $runId
	if ($failure.failures[0].message -cne 'fixture failure' -or $failure.exit_code -ne 3 -or $failure.truncated -or $failure.Contains('environment')) {
		throw 'Rift failure metadata was lost or unrelated summary content was retained.'
	}
	$summary.failures = @(1..9 | ForEach-Object { [ordered]@{ code = 'probe_failed'; stage = 'preflight'; message = ('x' * 5000) } })
	$summary | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $summaryPath -Encoding utf8
	$failure = Get-RiftFailureEvidence -Root $fixtureRoot -RunId $runId
	if (-not $failure.truncated -or $failure.failures.Count -ne 8 -or $failure.failures[0].message.Length -ne 4096) {
		throw 'Rift failure metadata did not preserve its truncation boundary.'
	}
	foreach ($invalidCase in @('run_id', 'identity', 'byte_limit')) {
		$requestedId = $runId
		if ($invalidCase -eq 'run_id') { $requestedId = '../outside' }
		if ($invalidCase -eq 'identity') {
			$summary.run_id = '20261005T081705Z-00000000'
			$summary | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $summaryPath -Encoding utf8
		}
		if ($invalidCase -eq 'byte_limit') { [IO.File]::WriteAllText($summaryPath, ('x' * 1048577)) }
		$rejected = $false
		try { Get-RiftFailureEvidence -Root $fixtureRoot -RunId $requestedId | Out-Null } catch { $rejected = $true }
		if (-not $rejected) { throw "Rift evidence accepted invalid $invalidCase." }
	}
} finally {
	$resolvedFixture = (Resolve-Path -LiteralPath $fixtureRoot).Path
	$relativeFixture = [IO.Path]::GetRelativePath($temporaryRoot, $resolvedFixture)
	if ([IO.Path]::IsPathRooted($relativeFixture) -or $relativeFixture -eq '..' -or $relativeFixture.StartsWith('..' + [IO.Path]::DirectorySeparatorChar)) {
		throw 'Refusing to remove an evidence fixture outside the temporary directory.'
	}
	Remove-Item -LiteralPath $resolvedFixture -Recurse -Force
}

Write-Host 'PASS Meridian compatibility evidence validation'
