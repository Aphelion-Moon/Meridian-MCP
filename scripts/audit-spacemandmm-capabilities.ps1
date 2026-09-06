[CmdletBinding()]
param(
	[switch]$Check,
	[string]$UpstreamPath
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$expectedRevision = '351ddc0ffb2439876d4565ce5130bb6b027ee605'
$repoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$registryPath = Join-Path $repoRoot 'spacemandmm-capabilities.json'
$registry = Get-Content -LiteralPath $registryPath -Raw | ConvertFrom-Json
$errors = [System.Collections.Generic.List[string]]::new()

function Add-AuditError {
	param([Parameter(Mandatory)][string]$Message)
	$errors.Add($Message)
}

if ($registry.schema_version -ne 1) {
	Add-AuditError "Unsupported registry schema $($registry.schema_version)."
}
if ($registry.spacemandmm_revision -ne $expectedRevision) {
	Add-AuditError "Capability registry revision $($registry.spacemandmm_revision) does not match $expectedRevision."
}

$localDeltaPath = Join-Path $repoRoot 'vendor/spacemandmm/local-delta.patch'
$localDelta = (Get-Content -LiteralPath $localDeltaPath -Raw).Replace("`r`n", "`n")
$localDeltaHash = [Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData([System.Text.Encoding]::UTF8.GetBytes($localDelta))).ToLowerInvariant()
$expectedDeltaHash = (Get-Content -LiteralPath (Join-Path $repoRoot 'vendor/spacemandmm/local-delta.sha256') -Raw).Trim()
if ($registry.local_patch -ne 'meridian-read-policy-v3' -or $localDeltaHash -ne $expectedDeltaHash) {
    Add-AuditError 'Local SpacemanDMM read-policy delta identity or hash does not match.'
}

$vendorRoot = Join-Path $repoRoot 'vendor/spacemandmm'
$sourceIdentity = Get-Content -LiteralPath (Join-Path $vendorRoot 'source-files.json') -Raw | ConvertFrom-Json -AsHashtable
if ($sourceIdentity.schema -ne 1 -or $sourceIdentity.upstream_revision -ne $expectedRevision -or $sourceIdentity.local_patch -ne $registry.local_patch -or $sourceIdentity.delta_sha256 -ne $expectedDeltaHash) {
	Add-AuditError 'Vendored source inventory identity does not match the pinned local delta.'
}
$observedSources = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
$sourceFiles = @(Get-ChildItem -LiteralPath (Join-Path $vendorRoot 'dreammaker'), (Join-Path $vendorRoot 'dmm-tools') -Recurse -File -Force) + @(Get-Item -LiteralPath (Join-Path $vendorRoot 'LICENSE') -Force)
foreach ($sourceFile in $sourceFiles) {
	$relativeSource = [IO.Path]::GetRelativePath($vendorRoot, $sourceFile.FullName).Replace('\', '/')
	[void]$observedSources.Add($relativeSource)
	$sourceText = [IO.File]::ReadAllText($sourceFile.FullName).Replace("`r`n", "`n")
	$sourceHash = [Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData([System.Text.Encoding]::UTF8.GetBytes($sourceText))).ToLowerInvariant()
	if (-not $sourceIdentity.files.ContainsKey($relativeSource) -or $sourceIdentity.files[$relativeSource] -cne $sourceHash) {
		Add-AuditError "Vendored source is changed or unrecorded: $relativeSource"
	}
}
foreach ($recordedSource in $sourceIdentity.files.Keys) {
	if (-not $observedSources.Contains($recordedSource)) {
		Add-AuditError "Recorded vendored source is missing: $recordedSource"
	}
}

$identities = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
foreach ($record in $registry.capabilities) {
	if (-not $identities.Add([string]$record.id)) {
		Add-AuditError "Duplicate capability id $($record.id)."
	}
	if ([string]::IsNullOrWhiteSpace([string]$record.verification)) {
		Add-AuditError "$($record.id) has no verification gate."
	}
	if ($record.disposition -eq 'excluded' -and [string]::IsNullOrWhiteSpace([string]$record.rationale)) {
		Add-AuditError "$($record.id) has no exclusion rationale."
	}
}

$evidence = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
foreach ($record in $registry.capabilities) {
	foreach ($item in $record.evidence) {
		[void]$evidence.Add([string]$item)
	}
}

$auditScope = 'registry and vendor integrity only; upstream source comparison not run'
if ($UpstreamPath) {
	$resolvedUpstream = (Resolve-Path -LiteralPath $UpstreamPath).Path
	$safeDirectory = $resolvedUpstream.Replace('\', '/')
	$actualRevision = (& git -c "safe.directory=$safeDirectory" -C $resolvedUpstream rev-parse HEAD)
	if ($LASTEXITCODE -ne 0) {
		throw "Unable to read the upstream revision at $resolvedUpstream."
	}
	$actualRevision = $actualRevision.Trim()
	if ($actualRevision -ne $expectedRevision) {
		Add-AuditError "Upstream checkout is $actualRevision, expected $expectedRevision."
	}

    & git -c "safe.directory=$safeDirectory" -C $resolvedUpstream diff --quiet HEAD --
    if ($LASTEXITCODE -ne 0) {
        Add-AuditError 'Upstream tracked sources differ from the pinned commit.'
    }
    . (Join-Path $PSScriptRoot 'spacemandmm-surface.ps1')
    $sources = Read-SpacemanSources $resolvedUpstream
    $surface = @(Get-SpacemanSurface $sources)
    foreach ($errorMessage in @(Get-SpacemanCoverageErrors $surface @($evidence))) {
        Add-AuditError $errorMessage
    }
    $localWire = [IO.File]::ReadAllText((Join-Path $repoRoot 'src/spaceman/debugger.rs'))
    foreach ($errorMessage in @(Get-SpacemanWireErrors $sources.wire $localWire)) {
        Add-AuditError $errorMessage
    }
    $auditScope = "$($surface.Count) source capabilities and debugger wire layouts"

}

if ($errors.Count -gt 0) {
	throw "SpacemanDMM capability audit failed:`n$($errors -join "`n")"
}

if ($Check) {
	Write-Output "Capability audit passed for $($registry.capabilities.Count) records at $expectedRevision ($auditScope)."
}
