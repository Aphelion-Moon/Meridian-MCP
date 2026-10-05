[CmdletBinding()]
param(
	[Parameter(Mandatory)][string]$TracyPath,
	[Parameter(Mandatory)][string]$ByondTracyPath,
	[Parameter(Mandatory)][string]$OutputDirectory,
	[Parameter(Mandatory)][string]$ManifestPath,
	[string]$BuildRoot,
	[string]$CpmSourceCache,
	[switch]$DebugHook
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$tracyRevision = '099df3de3dc37eca4712c06b8320fb9c53596edd'
$byondTracyRevision = 'd1ec404737b04b1ea73d6df4a1b477deacdb1900'
$tracy = (Resolve-Path -LiteralPath $TracyPath).Path
$byondTracy = (Resolve-Path -LiteralPath $ByondTracyPath).Path

function Assert-Revision([string]$Path, [string]$Expected, [string]$Name) {
	$actual = (& git -C $Path rev-parse HEAD).Trim()
	if ($LASTEXITCODE -ne 0 -or $actual -ne $Expected) {
		throw "$Name checkout must be at $Expected; found $actual"
	}
	$changes = @(& git -C $Path status --porcelain --untracked-files=no)
	if ($LASTEXITCODE -ne 0 -or $changes.Count -ne 0) {
		throw "$Name checkout must be unmodified before owned patches are applied to a private copy."
	}
}

function Invoke-OwnedPatch([string]$SourceRoot, [string]$PatchPath, [string]$Name) {
	$applyRoot = [IO.Path]::GetFullPath($SourceRoot)
	$applyArguments = @()
	$enclosingRepository = (& git -C $applyRoot rev-parse --show-toplevel 2>$null)
	if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($enclosingRepository)) {
		$repositoryRoot = [IO.Path]::GetFullPath($enclosingRepository.Trim())
		if (-not $applyRoot.Equals($repositoryRoot, [StringComparison]::OrdinalIgnoreCase)) {
			$sourcePrefix = [IO.Path]::GetRelativePath($repositoryRoot, $applyRoot).Replace('\', '/')
			if ($sourcePrefix.StartsWith('../', [StringComparison]::Ordinal) -or $sourcePrefix -eq '..') {
				throw "$Name source root escaped its enclosing repository."
			}
			$applyRoot = $repositoryRoot
			$applyArguments += "--directory=$sourcePrefix"
		}
	}
	Push-Location $applyRoot
	try {
		& git apply @applyArguments --check $PatchPath
		if ($LASTEXITCODE -ne 0) { throw "$Name did not pass git apply --check." }
		& git apply @applyArguments $PatchPath
		if ($LASTEXITCODE -ne 0) { throw "$Name did not apply cleanly." }
	} finally {
		Pop-Location
	}
}

Assert-Revision $tracy $tracyRevision 'Tracy'
Assert-Revision $byondTracy $byondTracyRevision 'byond-tracy'

if ([string]::IsNullOrWhiteSpace($BuildRoot)) {
	$BuildRoot = if ($IsWindows) { 'C:\mtb\meridian-tracy' } else { '/tmp/meridian-tracy-build' }
}
if ([string]::IsNullOrWhiteSpace($CpmSourceCache)) {
	$CpmSourceCache = if ($IsWindows) { 'C:\cpm' } else { '/tmp/meridian-tracy-cpm' }
}
$build = [IO.Path]::GetFullPath($BuildRoot)
$output = [IO.Path]::GetFullPath($OutputDirectory)
$manifest = [IO.Path]::GetFullPath($ManifestPath)

$cmakeCommand = Get-Command cmake -ErrorAction SilentlyContinue
if ($null -eq $cmakeCommand -and $IsWindows) {
	$vswhere = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe'
	$vsRoot = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath).Trim()
	$bundledCmake = Join-Path $vsRoot 'Common7/IDE/CommonExtensions/Microsoft/CMake/CMake/bin/cmake.exe'
	if (Test-Path -LiteralPath $bundledCmake) { $cmakeCommand = Get-Item -LiteralPath $bundledCmake }
}
if ($null -eq $cmakeCommand) { throw 'CMake 3.25 or newer is required.' }
$cmake = if ($cmakeCommand -is [System.Management.Automation.ApplicationInfo]) { $cmakeCommand.Source } else { $cmakeCommand.FullName }

New-Item -ItemType Directory -Force -Path $build, $CpmSourceCache | Out-Null
$tracyClockPatch = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../helpers/tracy/tracy-clock-access.patch')).Path
$tracySource = [IO.Path]::GetFullPath((Join-Path $build 'tracy-source'))
$expectedPrivatePrefix = $build.TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
if (-not $tracySource.StartsWith($expectedPrivatePrefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'Private Tracy source escaped the build root.' }
if (Test-Path -LiteralPath $tracySource) { Remove-Item -LiteralPath $tracySource -Recurse -Force }
New-Item -ItemType Directory -Force -Path $tracySource | Out-Null
Get-ChildItem -LiteralPath $tracy -Force | Where-Object Name -ne '.git' | Copy-Item -Destination $tracySource -Recurse -Force
Invoke-OwnedPatch $tracySource $tracyClockPatch 'The Meridian Tracy clock-access patch'

& $cmake -S (Join-Path $PSScriptRoot '../helpers/tracy') -B $build "-DTRACY_SOURCE_DIR=$tracySource" "-DCPM_SOURCE_CACHE=$CpmSourceCache" -DBUILD_TESTING=ON
if ($LASTEXITCODE -ne 0) { throw 'The pinned Tracy helper configure failed.' }
& $cmake --build $build --config Release --target meridian-tracy-helper meridian_tracy_protocol_tests meridian_tracy_query_tests meridian_tracy_validation_tests meridian_tracy_session_tests
if ($LASTEXITCODE -ne 0) { throw 'The pinned Tracy helper build failed.' }
$ctest = Join-Path (Split-Path -Parent $cmake) 'ctest.exe'
if (-not $IsWindows) { $ctest = (Get-Command ctest -ErrorAction Stop).Source }
& $ctest --test-dir $build -C Release --output-on-failure -R '^meridian_'
if ($LASTEXITCODE -ne 0) { throw 'The Meridian-owned Tracy helper tests failed.' }

$entries = @()
if (Test-Path -LiteralPath $manifest) {
	$existingManifest = Get-Content -LiteralPath $manifest -Raw | ConvertFrom-Json
	if ($existingManifest.schema_version -ne 2) { throw 'Tracy helpers can only merge into a schema-v2 helper manifest.' }
	$entries = @($existingManifest.helpers | Where-Object { $_.id -notin @('tracy-server-helper', 'byond-tracy') })
}
$helperPlatform = if ($IsWindows) { 'windows' } elseif ($IsLinux) { 'linux' } else { throw 'Unsupported helper platform.' }
$helperName = if ($IsWindows) { 'meridian-tracy-helper.exe' } else { 'meridian-tracy-helper' }
$helperSource = if ($IsWindows) { Join-Path $build "Release/$helperName" } else { Join-Path $build $helperName }
$helperTarget = Join-Path $output "helpers/bin/$helperPlatform-x86_64/$helperName"
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $helperTarget) | Out-Null
Copy-Item -LiteralPath $helperSource -Destination $helperTarget -Force
$entries += [ordered]@{
	id = 'tracy-server-helper'
	platform = $helperPlatform
	target_arch = 'x86_64'
	path = [IO.Path]::GetRelativePath((Split-Path -Parent $manifest), $helperTarget).Replace('\', '/')
	sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $helperTarget).Hash.ToLowerInvariant()
	source_revision = $tracyRevision
	patch_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $tracyClockPatch).Hash.ToLowerInvariant()
	protocol_version = 82
}

$hookName = if ($IsWindows) { 'prof.dll' } else { 'libprof.so' }
$hookTarget = Join-Path $output "helpers/bin/$helperPlatform-x86/$hookName"
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $hookTarget) | Out-Null
$hookBuildRoot = Join-Path $build 'byond-tracy-x86'
New-Item -ItemType Directory -Force -Path $hookBuildRoot | Out-Null
$hookBuild = Join-Path $hookBuildRoot $hookName
$hookSource = Join-Path $hookBuildRoot 'prof.c'
Copy-Item -LiteralPath (Join-Path $byondTracy 'prof.c') -Destination $hookSource -Force
$hookPatches = @(
	[ordered]@{ name = 'empty_queue'; path = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../helpers/tracy/byond-tracy-empty-queue.patch')).Path },
	[ordered]@{ name = 'queue_health'; path = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../helpers/tracy/byond-tracy-health.patch')).Path }
)
foreach ($ownedPatch in $hookPatches) {
	Invoke-OwnedPatch $hookBuildRoot $ownedPatch.path "The Meridian byond-tracy $($ownedPatch.name) patch"
}
if ($IsWindows) {
	$vswhere = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe'
	$vsRoot = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath).Trim()
	$developerCommand = Join-Path $vsRoot 'Common7/Tools/VsDevCmd.bat'
	$developerEnvironment = & $env:ComSpec /d /s /c "`"$developerCommand`" -no_logo -arch=x86 -host_arch=x64 >nul && set"
	if ($LASTEXITCODE -ne 0) { throw 'Could not initialize the x86 MSVC environment.' }
	foreach ($line in $developerEnvironment) {
		$separator = $line.IndexOf('=')
		if ($separator -gt 0) { Set-Item -LiteralPath "Env:$($line.Substring(0, $separator))" -Value $line.Substring($separator + 1) }
	}
	Push-Location $hookBuildRoot
	try {
		$hookArguments = @('/nologo', '/std:c11', '/O2', '/LD', $(if ($DebugHook) { '/DUTRACY_DEBUG' } else { '/DNDEBUG' }), "/I$byondTracy", $hookSource, 'ws2_32.lib', "/Fe:$hookBuild")
		& cl.exe @hookArguments
	} finally {
		Pop-Location
	}
} elseif ($IsLinux) {
	$hookArguments = @('-std=c11', '-m32', '-shared', '-fPIC', '-Ofast', '-s', $(if ($DebugHook) { '-DUTRACY_DEBUG' } else { '-DNDEBUG' }), "-I$byondTracy", $hookSource, '-pthread', '-o', $hookBuild)
	& gcc @hookArguments
}
if ($LASTEXITCODE -ne 0) { throw 'The pinned 32-bit byond-tracy hook build failed.' }
Copy-Item -LiteralPath $hookBuild -Destination $hookTarget -Force
$entries += [ordered]@{
	id = 'byond-tracy'
	platform = $helperPlatform
	target_arch = 'x86'
	path = [IO.Path]::GetRelativePath((Split-Path -Parent $manifest), $hookTarget).Replace('\', '/')
	sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $hookTarget).Hash.ToLowerInvariant()
	source_revision = $byondTracyRevision
	protocol_version = 82
	byond_min_version = '516.1685'
	byond_max_version = '516.1687'
	patches = @($hookPatches | ForEach-Object { [ordered]@{ name = $_.name; patch_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $_.path).Hash.ToLowerInvariant() } })
	telemetry = [ordered]@{
		queue_capacity = $true
		queue_depth = $true
		queue_high_water = $true
		queue_tail_refresh_count = $true
		queue_saturation_count = $true
		queue_dropped_events = $true
		produced_events = $true
		consumed_events = $true
		last_producer_progress = $true
		prologue_validated = $true
		module_relative_offset = $true
		offset_table_identity = $true
	}
}

$licenseRoot = Join-Path $output 'helpers/licenses'
New-Item -ItemType Directory -Force -Path $licenseRoot | Out-Null
Copy-Item -LiteralPath (Join-Path $tracy 'LICENSE') -Destination (Join-Path $licenseRoot 'tracy-LICENSE') -Force
Copy-Item -LiteralPath (Join-Path $byondTracy 'LICENSE') -Destination (Join-Path $licenseRoot 'byond-tracy-LICENSE') -Force
Get-ChildItem -LiteralPath (Join-Path $build 'third-party-notices') -File | ForEach-Object {
	Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $licenseRoot $_.Name) -Force
}
# These complete license-header ranges belong to the exact pinned revisions above.
$embeddedNotices = @(
	@{ root = $tracy; path = 'public/common/tracy_lz4.cpp'; first = 1; last = 33; name = 'tracy-lz4-NOTICE' },
	@{ root = $tracy; path = 'public/common/tracy_lz4hc.cpp'; first = 1; last = 33; name = 'tracy-lz4hc-NOTICE' },
	@{ root = $tracy; path = 'server/tracy_robin_hood.h'; first = 1; last = 31; name = 'tracy-robin-hood-NOTICE' },
	@{ root = $tracy; path = 'server/tracy_xxhash.h'; first = 1; last = 34; name = 'tracy-xxhash-NOTICE' },
	@{ root = $tracy; path = 'server/tracy_pdqsort.h'; first = 1; last = 20; name = 'tracy-pdqsort-NOTICE' },
	@{ root = $byondTracy; path = 'prof.c'; first = 310; last = 338; name = 'byond-tracy-lz4-NOTICE' }
)
foreach ($notice in $embeddedNotices) {
	$lines = @(Get-Content -LiteralPath (Join-Path $notice.root $notice.path) -TotalCount $notice.last)
	if ($lines.Count -ne $notice.last) { throw "The pinned source notice is incomplete: $($notice.path)" }
	$text = [string]::Join("`n", $lines[($notice.first - 1)..($notice.last - 1)]) + "`n"
	[IO.File]::WriteAllText((Join-Path $licenseRoot $notice.name), $text, [Text.UTF8Encoding]::new($false))
}

New-Item -ItemType Directory -Force -Path (Split-Path -Parent $manifest) | Out-Null
$document = [ordered]@{ schema_version = 2; helpers = $entries }
[IO.File]::WriteAllText($manifest, (($document | ConvertTo-Json -Depth 8) + [Environment]::NewLine), [Text.UTF8Encoding]::new($false))
[pscustomobject]@{ helper_manifest = $manifest; helper = $helperTarget; hook = $hookTarget } | ConvertTo-Json
