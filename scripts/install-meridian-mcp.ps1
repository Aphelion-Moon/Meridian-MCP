[CmdletBinding()]
param(
	[Parameter(Mandatory)][string]$BinaryPath,
	[Parameter(Mandatory)][string]$HelperManifestPath,
	[Parameter(Mandatory)][string]$AuxtoolsRoot,
	[Parameter(Mandatory)][string]$DestinationRoot,
	[string]$MemoryHelperDirectory,
	[string]$InstalledName = 'meridian-mcp-spacemandmm-20260824.exe',
	[string[]]$WorkspaceRoots = @(),
	[string[]]$RepositoryRoots = @(),
	[string]$StateDirectory,
	[switch]$Development,
	[switch]$EnableTracy
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$binary = (Resolve-Path -LiteralPath $BinaryPath).Path
$manifestPath = (Resolve-Path -LiteralPath $HelperManifestPath).Path
$auxRoot = (Resolve-Path -LiteralPath $AuxtoolsRoot).Path
$destination = [IO.Path]::GetFullPath($DestinationRoot)
New-Item -ItemType Directory -Force -Path $destination | Out-Null
$resolvedWorkspaceRoots = @($WorkspaceRoots | ForEach-Object {
	if (-not (Test-Path -LiteralPath $_ -PathType Container)) { throw "Workspace root does not exist: $_" }
	(Resolve-Path -LiteralPath $_).Path
} | Select-Object -Unique)
$resolvedRepositoryRoots = @($RepositoryRoots | ForEach-Object {
	if (-not (Test-Path -LiteralPath $_ -PathType Container)) { throw "Repository root does not exist: $_" }
	(Resolve-Path -LiteralPath $_).Path
} | Select-Object -Unique)
$resolvedStateDirectory = $null
if ($Development) {
	if ([string]::IsNullOrWhiteSpace($StateDirectory)) { throw 'Development mode requires StateDirectory.' }
	$resolvedStateDirectory = [IO.Path]::GetFullPath($StateDirectory)
	foreach ($root in $resolvedWorkspaceRoots) {
		$relative = [IO.Path]::GetRelativePath($root, $resolvedStateDirectory)
		if ($relative -eq '.' -or (-not $relative.StartsWith('..' + [IO.Path]::DirectorySeparatorChar) -and $relative -ne '..')) { throw 'StateDirectory must be outside every workspace root.' }
	}
	New-Item -ItemType Directory -Force -Path $resolvedStateDirectory | Out-Null
}
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$platform = if ($IsWindows) { 'windows' } elseif ($IsLinux) { 'linux' } else { throw 'Unsupported installation platform.' }
$hostArchitecture = 'x86_64'
$normalizedHelpers = @($manifest.helpers | ForEach-Object {
	if ($manifest.schema_version -eq 1) {
		$parts = $_.platform.Split('-', 2)
		[pscustomobject]@{ id = 'dmdoc'; platform = $parts[0]; target_arch = $parts[1]; path = $_.path; sha256 = $_.sha256; source_revision = $_.source_revision; protocol_version = $null; byond_min_version = $null; byond_max_version = $null; patch_sha256 = $null; patches = @(); telemetry = $null }
	} elseif ($manifest.schema_version -eq 2) {
		[pscustomobject]@{
			id = $_.id
			platform = $_.platform
			target_arch = $_.target_arch
			path = $_.path
			sha256 = $_.sha256
			source_revision = $_.source_revision
			protocol_version = if ($null -ne $_.PSObject.Properties['protocol_version']) { $_.protocol_version } else { $null }
			byond_min_version = if ($null -ne $_.PSObject.Properties['byond_min_version']) { $_.byond_min_version } else { $null }
			byond_max_version = if ($null -ne $_.PSObject.Properties['byond_max_version']) { $_.byond_max_version } else { $null }
			patch_sha256 = if ($null -ne $_.PSObject.Properties['patch_sha256']) { $_.patch_sha256 } else { $null }
			patches = @(if ($null -ne $_.PSObject.Properties['patches']) { $_.patches })
			telemetry = if ($null -ne $_.PSObject.Properties['telemetry']) { $_.telemetry } else { $null }
		}
	} else {
		throw "Unsupported helper manifest schema $($manifest.schema_version)."
	}
})
$selectedHelpers = @($normalizedHelpers | Where-Object platform -eq $platform)
$dmdocHelpers = @($selectedHelpers | Where-Object { $_.id -eq 'dmdoc' -and $_.target_arch -eq $hostArchitecture })
if ($dmdocHelpers.Count -ne 1) { throw "Expected exactly one $platform-$hostArchitecture dmdoc helper." }
if ($EnableTracy) {
	$tracyHelpers = @($selectedHelpers | Where-Object { $_.id -eq 'tracy-server-helper' -and $_.target_arch -eq $hostArchitecture })
	$tracyHooks = @($selectedHelpers | Where-Object { $_.id -eq 'byond-tracy' -and $_.target_arch -eq 'x86' })
	if ($tracyHelpers.Count -ne 1 -or $tracyHooks.Count -ne 1) { throw "Tracy installation requires one $platform-$hostArchitecture server helper and one $platform-x86 hook." }
}
foreach ($helper in $selectedHelpers) {
	$helperSource = [IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $manifestPath) $helper.path))
	if ((Get-FileHash -Algorithm SHA256 -LiteralPath $helperSource).Hash.ToLowerInvariant() -ne $helper.sha256) { throw "Source $($helper.id) helper hash mismatch." }
}

function Install-File([string]$Source, [string]$Target) {
	$parent = Split-Path -Parent $Target
	New-Item -ItemType Directory -Force -Path $parent | Out-Null
	$temporary = Join-Path $parent ('.install-' + [Guid]::NewGuid().ToString('N') + '.tmp')
	try {
		Copy-Item -LiteralPath $Source -Destination $temporary
		$sourceHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Source).Hash
		$temporaryHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $temporary).Hash
		if ($sourceHash -ne $temporaryHash) { throw "Staged hash mismatch for $Target" }
		Move-Item -LiteralPath $temporary -Destination $Target -Force
	} finally {
		Remove-Item -LiteralPath $temporary -Force -ErrorAction SilentlyContinue
	}
}

$installedBinary = Join-Path $destination $InstalledName
if (-not [string]::IsNullOrWhiteSpace($MemoryHelperDirectory)) {
	$memoryRoot = (Resolve-Path -LiteralPath $MemoryHelperDirectory).Path
	$memoryManifest = Get-Content -LiteralPath (Join-Path $memoryRoot 'manifest.json') -Raw | ConvertFrom-Json
	$memoryEntries = @($memoryManifest.helpers)
	if ($memoryManifest.schema_version -ne 2 -or $memoryEntries.Count -ne 1) { throw 'Invalid native memory manifest.' }
	$memoryEntry = $memoryEntries[0]
	if ($memoryEntry.id -ne 'auxtools-memory' -or $memoryEntry.platform -ne 'windows' -or $memoryEntry.target_arch -ne 'x86' -or $memoryEntry.path -ne 'debug_server.dll' -or $memoryEntry.source_revision -ne '889006e334570a426f35c0a2f579c08d3d7b2186' -or $memoryEntry.protocol_version -ne 1 -or $memoryEntry.byond_min_version -ne '516.1687' -or $memoryEntry.byond_max_version -ne '516.1687') { throw 'Unexpected native memory helper identity.' }
	if (@($memoryEntry.patches).Count -ne 3) { throw 'Native memory source overlays are missing.' }
	foreach ($name in @('accounting.rs','mem_profiler.rs','protocol.patch')) {
		$entry = @($memoryEntry.patches | Where-Object name -eq $name)
		$expectedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $PSScriptRoot "../helpers/auxtools-memory/$name")).Hash.ToLowerInvariant()
		if ($entry.Count -ne 1 -or $entry[0].patch_sha256 -ne $expectedHash) { throw 'Native memory source overlay mismatch.' }
	}
	if ((Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $memoryRoot 'debug_server.dll')).Hash.ToLowerInvariant() -ne $memoryEntry.sha256) { throw 'Native memory helper checksum mismatch.' }
}
Install-File $binary $installedBinary
$installedHelpers = @()
foreach ($helper in $selectedHelpers) {
	$helperSource = [IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $manifestPath) $helper.path))
	$helperTarget = Join-Path $destination "helpers/bin/$platform-$($helper.target_arch)/$([IO.Path]::GetFileName($helperSource))"
	Install-File $helperSource $helperTarget
	$installedEntry = [ordered]@{
		id = $helper.id
		platform = $helper.platform
		target_arch = $helper.target_arch
		path = "bin/$platform-$($helper.target_arch)/$([IO.Path]::GetFileName($helperTarget))"
		sha256 = $helper.sha256
		source_revision = $helper.source_revision
	}
	if ($null -ne $helper.protocol_version) { $installedEntry.protocol_version = $helper.protocol_version }
	if ($null -ne $helper.byond_min_version) { $installedEntry.byond_min_version = $helper.byond_min_version }
	if ($null -ne $helper.byond_max_version) { $installedEntry.byond_max_version = $helper.byond_max_version }
	if ($null -ne $helper.patch_sha256) { $installedEntry.patch_sha256 = $helper.patch_sha256 }
	if (@($helper.patches).Count -gt 0) { $installedEntry.patches = @($helper.patches) }
	if ($null -ne $helper.telemetry) { $installedEntry.telemetry = $helper.telemetry }
	$installedHelpers += $installedEntry
}
$auxSource = Join-Path $auxRoot 'helpers/auxtools/v2.3.7/debug_server.dll'
$auxTarget = Join-Path $destination 'helpers/auxtools/v2.3.7/debug_server.dll'
Install-File $auxSource $auxTarget
if ((Get-FileHash -Algorithm SHA256 -LiteralPath $auxTarget).Hash.ToLowerInvariant() -ne 'b188999ac58a0e0171b015c39a403ab7da2f37ddb8ac3817a078f5bce02a8be7') { throw 'Installed auxtools hash mismatch.' }
if (-not [string]::IsNullOrWhiteSpace($MemoryHelperDirectory)) {
	foreach ($name in @('debug_server.dll','manifest.json','LICENSE')) {
		Install-File (Join-Path $memoryRoot $name) (Join-Path $destination "helpers/auxtools-memory/$name")
	}
}

$installedManifestPath = Join-Path $destination 'helpers/manifest.json'
$installedManifest = [ordered]@{ schema_version = 2; helpers = $installedHelpers }
$manifestTemporary = $installedManifestPath + '.tmp-' + [Guid]::NewGuid().ToString('N')
try {
	[IO.File]::WriteAllText($manifestTemporary, (($installedManifest | ConvertTo-Json -Depth 5) + [Environment]::NewLine), [Text.UTF8Encoding]::new($false))
	Move-Item -LiteralPath $manifestTemporary -Destination $installedManifestPath -Force
} finally {
	Remove-Item -LiteralPath $manifestTemporary -Force -ErrorAction SilentlyContinue
}
$configurationEnvironment = [ordered]@{}
if ($resolvedWorkspaceRoots.Count -gt 0) { $configurationEnvironment.MERIDIAN_MCP_ROOTS = [string]::Join([IO.Path]::PathSeparator, $resolvedWorkspaceRoots) }
if ($resolvedRepositoryRoots.Count -gt 0) { $configurationEnvironment.MERIDIAN_MCP_REPOSITORIES = [string]::Join([IO.Path]::PathSeparator, $resolvedRepositoryRoots) }
if ($Development) { $configurationEnvironment.MERIDIAN_MCP_STATE_DIR = $resolvedStateDirectory }
[pscustomobject]@{
	binary = $installedBinary
	helper_manifest = $installedManifestPath
	auxtools = $auxTarget
	tracy_enabled = [bool]$EnableTracy
	binary_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $installedBinary).Hash.ToLowerInvariant()
	workspace_roots = $resolvedWorkspaceRoots
	repository_roots = $resolvedRepositoryRoots
	state_directory = $resolvedStateDirectory
	configuration_environment = $configurationEnvironment
} | ConvertTo-Json
