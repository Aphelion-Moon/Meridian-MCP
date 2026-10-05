[CmdletBinding()]
param(
	[Parameter(Mandatory)][string]$UpstreamPath,
	[Parameter(Mandatory)][string]$OutputDirectory,
	[Parameter(Mandatory)][string]$ManifestPath
)

$ErrorActionPreference = 'Stop'
$revision = '351ddc0ffb2439876d4565ce5130bb6b027ee605'
$upstream = (Resolve-Path -LiteralPath $UpstreamPath).Path
$actualRevision = (& git -C $upstream rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $actualRevision -ne $revision) { throw "SpacemanDMM checkout must be at $revision; found $actualRevision" }
$toolchain = if ($IsWindows) { '1.95.0-x86_64-pc-windows-msvc' } elseif ($IsLinux) { '1.95.0-x86_64-unknown-linux-gnu' } else { throw 'Unsupported helper platform.' }
& rustup run $toolchain cargo build --locked --release -p dmdoc --manifest-path (Join-Path $upstream 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'The exact dmdoc helper build failed.' }
$platform = if ($IsWindows) { 'windows-x86_64' } elseif ($IsLinux) { 'linux-x86_64' } else { throw 'Unsupported helper platform.' }
$binaryName = if ($IsWindows) { 'dmdoc.exe' } else { 'dmdoc' }
$source = Join-Path $upstream "target/release/$binaryName"
$destinationRoot = [IO.Path]::GetFullPath($OutputDirectory)
$destination = Join-Path $destinationRoot "helpers/bin/$platform/$binaryName"
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destination) | Out-Null
Copy-Item -LiteralPath $source -Destination $destination -Force
$licenseRoot = Join-Path $destinationRoot 'helpers/licenses'
New-Item -ItemType Directory -Force -Path $licenseRoot | Out-Null
Copy-Item -LiteralPath (Join-Path $upstream 'LICENSE') -Destination (Join-Path $licenseRoot 'SpacemanDMM-LICENSE') -Force
$hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $destination).Hash.ToLowerInvariant()
$relative = [IO.Path]::GetRelativePath((Split-Path -Parent ([IO.Path]::GetFullPath($ManifestPath))), $destination).Replace('\', '/')
$platformParts = $platform.Split('-', 2)
$manifest = [ordered]@{
	schema_version = 2
	helpers = @([ordered]@{
		id = 'dmdoc'
		platform = $platformParts[0]
		target_arch = $platformParts[1]
		path = $relative
		sha256 = $hash
		source_revision = $revision
	})
}
$json = $manifest | ConvertTo-Json -Depth 5
$manifestParent = Split-Path -Parent ([IO.Path]::GetFullPath($ManifestPath))
New-Item -ItemType Directory -Force -Path $manifestParent | Out-Null
[IO.File]::WriteAllText([IO.Path]::GetFullPath($ManifestPath), $json + [Environment]::NewLine, [Text.UTF8Encoding]::new($false))
