[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$SourceRoot,
    [Parameter(Mandatory)][string]$BuildRoot,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [switch]$Offline
)
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'The native memory helper is Windows-only.' }
$revision = '889006e334570a426f35c0a2f579c08d3d7b2186'
$source = (Resolve-Path -LiteralPath $SourceRoot).Path
if ((& git -C $source rev-parse HEAD).Trim() -ne $revision) { throw 'Unexpected Auxtools source revision.' }
$build = [IO.Path]::GetFullPath($BuildRoot)
$output = [IO.Path]::GetFullPath($OutputDirectory)
$overlays = (Resolve-Path (Join-Path $PSScriptRoot '../helpers/auxtools-memory')).Path
New-Item -ItemType Directory -Force -Path $build, $output | Out-Null
$owned = Join-Path $build ('source-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $owned | Out-Null
$archive = Join-Path $owned 'source.tar'
& git -C $source archive --format=tar --output=$archive $revision
if ($LASTEXITCODE -ne 0) { throw 'Could not export pinned source.' }
& tar -xf $archive -C $owned
if ($LASTEXITCODE -ne 0) { throw 'Could not extract pinned source.' }
Remove-Item -LiteralPath $archive
# Initialize this private archive so git apply cannot discover the parent MCP repository.
& git -C $owned init --quiet
if ($LASTEXITCODE -ne 0) { throw 'Could not initialize private patch root.' }
& git -C $owned apply --check (Join-Path $overlays 'protocol.patch')
if ($LASTEXITCODE -ne 0) { throw 'Protocol patch check failed.' }
& git -C $owned apply (Join-Path $overlays 'protocol.patch')
if ($LASTEXITCODE -ne 0) { throw 'Protocol patch failed.' }
foreach ($name in @('accounting.rs','mem_profiler.rs')) {
    Copy-Item -LiteralPath (Join-Path $overlays $name) -Destination (Join-Path $owned "debug_server/src/$name")
}
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$vs = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath).Trim()
Import-Module (Join-Path $vs 'Common7/Tools/Microsoft.VisualStudio.DevShell.dll')
Enter-VsDevShell -VsInstallPath $vs -SkipAutomaticLocation -DevCmdArguments '-arch=x86 -host_arch=x64'
$arguments = @('+1.95.0','build','--locked','--manifest-path',(Join-Path $owned 'Cargo.toml'),'-p','debug_server','--release','--target','i686-pc-windows-msvc','--target-dir',(Join-Path $build 'target'))
if ($Offline) { $arguments += '--offline' }
& cargo @arguments
if ($LASTEXITCODE -ne 0) { throw 'Native memory helper build failed.' }
$binary = Join-Path $build 'target/i686-pc-windows-msvc/release/debug_server.dll'
Copy-Item -LiteralPath $binary -Destination (Join-Path $output 'debug_server.dll')
Copy-Item -LiteralPath (Join-Path $overlays 'LICENSE') -Destination (Join-Path $output 'LICENSE')
Copy-Item -LiteralPath (Join-Path $owned 'debug_server/LICENSE') -Destination (Join-Path $output 'LICENSE-debug-server')
Copy-Item -LiteralPath (Join-Path $owned 'README.md') -Destination (Join-Path $output 'UPSTREAM-README.md')
$patches = @('accounting.rs','mem_profiler.rs','protocol.patch') | ForEach-Object {
    @{ name=$_; patch_sha256=(Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $overlays $_)).Hash.ToLowerInvariant() }
}
$manifest = @{schema_version=2;helpers=@(@{
    id='auxtools-memory';platform='windows';target_arch='x86';path='debug_server.dll'
    sha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $binary).Hash.ToLowerInvariant()
    source_revision=$revision;protocol_version=1;byond_min_version='516.1687';byond_max_version='516.1687';patches=@($patches)
})}
[IO.File]::WriteAllText((Join-Path $output 'manifest.json'), ($manifest | ConvertTo-Json -Depth 8) + "`n", [Text.UTF8Encoding]::new($false))
Write-Output "Native memory helper package: $output"
