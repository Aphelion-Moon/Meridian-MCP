use std::path::Path;
use std::process::Command;

fn marker_writer_script(delay_ms: u64) -> String {
    format!(
        r#"param([string]$Marker)
# Initialize the cmdlet before reporting that the fixture can accept work.
Start-Sleep -Milliseconds 1
$fixtureDeadline = [Diagnostics.Stopwatch]::StartNew()
[IO.File]::WriteAllText("$Marker.boot", 'BOOT')
while (-not [IO.File]::Exists("$Marker.release")) {{
    if ($fixtureDeadline.Elapsed.TotalSeconds -ge 60) {{ throw 'Marker writer was never released.' }}
    Start-Sleep -Milliseconds 10
}}
Start-Sleep -Milliseconds {delay_ms}
[IO.File]::WriteAllText($Marker, 'READY')
# The harness owns termination; scheduler delays must not make the writer exit
# before readiness can be observed. Bound its lifetime if the harness crashes.
while ($fixtureDeadline.Elapsed.TotalSeconds -lt 60) {{ Start-Sleep -Milliseconds 100 }}
"#
    )
}

fn release_marker_writer(process: &str, marker: &Path) -> String {
    format!(
        r#"$bootstrapDeadline = [Diagnostics.Stopwatch]::StartNew()
while (-not [IO.File]::Exists('{marker}.boot')) {{
    {process}.Refresh()
    if ({process}.HasExited) {{ throw 'Marker writer exited during initialization.' }}
    if ($bootstrapDeadline.Elapsed.TotalSeconds -ge 30) {{ throw 'Marker writer did not finish initialization.' }}
    Start-Sleep -Milliseconds 25
}}
[IO.File]::WriteAllText('{marker}.release', 'RELEASE')"#,
        marker = marker.display()
    )
}

fn run_module_harness(name: &str, body: &str) -> std::process::Output {
    let root = std::env::temp_dir().join(format!(
        "meridian-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let module = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/process-readiness.psm1");
    let harness = root.join("harness.ps1");
    std::fs::write(
        &harness,
        format!(
            "$ErrorActionPreference = 'Stop'\nImport-Module -Force '{}'\n{}\n",
            module.display(),
            body
        ),
    )
    .unwrap();
    let output = Command::new("pwsh")
        .args(["-NoLogo", "-NoProfile", "-File"])
        .arg(&harness)
        .output()
        .expect("PowerShell should launch");
    std::fs::remove_dir_all(root).unwrap();
    output
}

#[test]
fn byond_version_identity_accepts_verified_linux_provenance_without_pe_metadata() {
    let output = run_module_harness(
        "byond-version-identity",
        r#"$linux = Get-ByondVersionIdentity -ExpectedVersion '516.1687' -ReportedFileVersion $null
if ($linux.version -ne '516.1687') { throw "Unexpected Linux version: $($linux.version)" }
if ($linux.verification -ne 'expected_input_only') { throw "Unexpected Linux verification: $($linux.verification)" }
$windows = Get-ByondVersionIdentity -ExpectedVersion '516.1687' -ReportedFileVersion '0.0.516.1687'
if ($windows.verification -ne 'file_metadata_match') { throw "Unexpected Windows verification: $($windows.verification)" }
try {
	Get-ByondVersionIdentity -ExpectedVersion '516.1687' -ReportedFileVersion '0.0.516.1686' | Out-Null
	throw 'A mismatched file version was accepted.'
} catch {
	if ($_.Exception.Message -eq 'A mismatched file version was accepted.') { throw }
}"#,
    );
    assert!(
        output.status.success(),
        "BYOND version identity failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn prototype_runtime_arguments_bind_the_synthetic_world_to_loopback() {
    let output = run_module_harness(
        "prototype-runtime-arguments",
        r#"$arguments = New-PrototypeDreamDaemonArguments -DmbPath 'fixture.dmb'
$expected = @('fixture.dmb', '0', '-ip', '127.0.0.1', '-trusted', '-log', 'dreamdaemon.world.log', '-close', '-verbose')
if ([string]::Join('|', $arguments) -ne [string]::Join('|', $expected)) {
	throw "Unexpected DreamDaemon arguments: $($arguments -join '|')"
}"#,
    );
    assert!(
        output.status.success(),
        "DreamDaemon argument construction failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn readiness_marker_wins_while_the_launched_process_is_still_running() {
    let root = std::env::temp_dir().join(format!(
        "meridian-process-readiness-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let child = root.join("delayed-marker.ps1");
    let marker = root.join("startup.marker");
    std::fs::write(&child, marker_writer_script(300)).unwrap();

    let module = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/process-readiness.psm1");
    let harness = root.join("harness.ps1");
    std::fs::write(
        &harness,
        format!(
            r#"$ErrorActionPreference = 'Stop'
Import-Module -Force '{}'
$startOptions = @{{}}
if ($IsWindows) {{ $startOptions.WindowStyle = 'Hidden' }}
$child = Start-Process -FilePath (Get-Process -Id $PID).Path -ArgumentList @('-NoLogo', '-NoProfile', '-File', '{}', '{}') -PassThru @startOptions
try {{
	{}
	$result = Wait-ProcessReadiness -Process $child -MarkerPath '{}' -ExpectedMarker 'READY' -TimeoutSeconds 3
	$child.Refresh()
	if ($result.status -ne 'ready') {{ throw "Unexpected status: $($result.status)" }}
	if ([IO.File]::ReadAllText('{}') -ne 'READY') {{ throw 'Readiness was reported before the marker was written.' }}
	if ($null -eq $result.elapsed_milliseconds -or $result.elapsed_milliseconds -lt 0) {{ throw 'Readiness elapsed time was not recorded.' }}
	if (@($result.samples).Count -lt 1) {{ throw 'Readiness samples were not recorded.' }}
	if ($null -eq $result.last_progress_milliseconds) {{ throw 'Last progress time was not recorded.' }}
	if ($null -ne $result.process_exit_code) {{ throw 'A running helper reported an exit code.' }}
	if ($child.HasExited) {{ throw 'The helper waited for process exit instead of readiness.' }}
}} finally {{
	Stop-Process -Id $child.Id -Force -ErrorAction SilentlyContinue
}}
"#,
            module.display(),
            child.display(),
            marker.display(),
            release_marker_writer("$child", &marker),
            marker.display(),
            marker.display()
        ),
    )
    .unwrap();

    let output = Command::new("pwsh")
        .args(["-NoLogo", "-NoProfile", "-File"])
        .arg(&harness)
        .output()
        .expect("PowerShell should launch");
    assert!(
        output.status.success(),
        "readiness helper failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn readiness_marker_can_arrive_after_the_launcher_exits() {
    let root = std::env::temp_dir().join(format!(
        "meridian-launcher-readiness-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let marker_writer = root.join("delayed-marker.ps1");
    let marker = root.join("startup.marker");
    std::fs::write(&marker_writer, marker_writer_script(500)).unwrap();

    let module = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/process-readiness.psm1");
    let harness = root.join("harness.ps1");
    std::fs::write(
        &harness,
        format!(
            r#"$ErrorActionPreference = 'Stop'
Import-Module -Force '{}'
$startOptions = @{{}}
if ($IsWindows) {{ $startOptions.WindowStyle = 'Hidden' }}
$launcher = Start-Process -FilePath (Get-Process -Id $PID).Path -ArgumentList @('-NoLogo', '-NoProfile', '-Command', 'Start-Sleep -Milliseconds 100') -PassThru @startOptions
$launcher.WaitForExit()
$markerWriter = Start-Process -FilePath (Get-Process -Id $PID).Path -ArgumentList @('-NoLogo', '-NoProfile', '-File', '{}', '{}') -PassThru @startOptions
try {{
	{}
	$result = Wait-ProcessReadiness -Process $launcher -MarkerPath '{}' -ExpectedMarker 'READY' -TimeoutSeconds 3
	if ($result.status -ne 'ready') {{ throw "Unexpected status: $($result.status)" }}
	if ([IO.File]::ReadAllText('{}') -ne 'READY') {{ throw 'Readiness was reported before the marker was written.' }}
	if (@($result.samples).Count -lt 1) {{ throw 'Launcher readiness samples were not recorded.' }}
	if ($null -eq $result.process_exit_code) {{ throw 'Exited launcher did not report an exit code.' }}
}} finally {{
	Stop-Process -Id $launcher.Id -Force -ErrorAction SilentlyContinue
	Stop-Process -Id $markerWriter.Id -Force -ErrorAction SilentlyContinue
}}
"#,
            module.display(),
            marker_writer.display(),
            marker.display(),
            release_marker_writer("$markerWriter", &marker),
            marker.display(),
            marker.display()
        ),
    )
    .unwrap();

    let output = Command::new("pwsh")
        .args(["-NoLogo", "-NoProfile", "-File"])
        .arg(&harness)
        .output()
        .expect("PowerShell should launch");
    assert!(
        output.status.success(),
        "readiness helper failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn readiness_timeout_retains_progress_samples_for_a_busy_process() {
    let root = std::env::temp_dir().join(format!(
        "meridian-progress-readiness-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let busy_child = root.join("busy-child.ps1");
    let marker = root.join("startup.marker");
    std::fs::write(
        &busy_child,
        "$value = 0\nwhile ($true) { $value = ($value + 1) % 1000000 }\n",
    )
    .unwrap();

    let module = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/process-readiness.psm1");
    let harness = root.join("harness.ps1");
    std::fs::write(
        &harness,
        format!(
            r#"$ErrorActionPreference = 'Stop'
Import-Module -Force '{}'
$startOptions = @{{}}
if ($IsWindows) {{ $startOptions.WindowStyle = 'Hidden' }}
$child = Start-Process -FilePath (Get-Process -Id $PID).Path -ArgumentList @('-NoLogo', '-NoProfile', '-File', '{}') -PassThru @startOptions
try {{
	$result = Wait-ProcessReadiness -Process $child -MarkerPath '{}' -ExpectedMarker 'READY' -TimeoutSeconds 2
	if ($result.status -ne 'timed_out') {{ throw "Unexpected status: $($result.status)" }}
	if ($result.elapsed_milliseconds -lt 1900) {{ throw 'Timeout elapsed time was not recorded.' }}
	if (@($result.samples).Count -lt 2) {{ throw 'Timeout did not retain bounded progress samples.' }}
	if ($result.last_progress_milliseconds -le 0) {{ throw 'Busy process progress was not detected.' }}
	if ($null -ne $result.process_exit_code) {{ throw 'Busy process reported an exit code.' }}
}} finally {{
	Stop-Process -Id $child.Id -Force -ErrorAction SilentlyContinue
}}
"#,
            module.display(),
            busy_child.display(),
            marker.display()
        ),
    )
    .unwrap();

    let output = Command::new("pwsh")
        .args(["-NoLogo", "-NoProfile", "-File"])
        .arg(&harness)
        .output()
        .expect("PowerShell should launch");
    assert!(
        output.status.success(),
        "readiness helper failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn readiness_timeout_records_final_progress_without_accepting_a_late_marker() {
    for publish_late_marker in [false, true] {
        let output = run_module_harness(
            "readiness-delayed-poll",
            &format!(
                r#"$marker = Join-Path $PSScriptRoot 'late.marker'
& (Get-Module 'process-readiness') {{
    param($lateMarker)
    $script:lateMarker = $lateMarker
    # Delay the real polling loop beyond its deadline while the observed
    # process consumes CPU. Keep the delay entirely inside this test module.
    function script:Start-Sleep {{
        param([int]$Milliseconds)
        $delay = [Diagnostics.Stopwatch]::StartNew()
        while ($delay.ElapsedMilliseconds -lt 1200) {{}}
        if ($script:lateMarker) {{ [IO.File]::WriteAllText($script:lateMarker, 'READY') }}
    }}
}} $(if ({publish_late_marker}) {{ $marker }} else {{ $null }})
$result = Wait-ProcessReadiness -Process ([Diagnostics.Process]::GetCurrentProcess()) -MarkerPath $marker -ExpectedMarker 'READY' -TimeoutSeconds 1
$result | ConvertTo-Json -Depth 5 -Compress
if ($result.status -ne 'timed_out') {{ throw 'A marker first observed after the deadline was accepted.' }}
if (@($result.samples).Count -lt 2) {{ throw 'The timeout omitted its final process sample.' }}
$first = $result.samples[0]
$last = $result.samples[-1]
if ($last.elapsed_milliseconds -lt 1000) {{ throw 'The final sample predates the deadline.' }}
if ($last.total_processor_milliseconds -le $first.total_processor_milliseconds) {{ throw 'The final sample lost real CPU progress.' }}
if ($result.last_progress_milliseconds -lt 1000) {{ throw 'Timeout classification lost the final observed progress.' }}
if ($last.marker_exists -ne {publish_late_marker}) {{ throw 'The final sample lost late-marker evidence.' }}
"#,
                publish_late_marker = if publish_late_marker {
                    "$true"
                } else {
                    "$false"
                },
            ),
        );
        assert!(
            output.status.success(),
            "delayed readiness poll failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn prototype_runtime_classification_distinguishes_control_boundary_and_progress() {
    let root = std::env::temp_dir().join(format!(
        "meridian-runtime-classification-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let module = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/process-readiness.psm1");
    let harness = root.join("harness.ps1");
    std::fs::write(
        &harness,
        format!(
            r#"$ErrorActionPreference = 'Stop'
Import-Module -Force '{}'
$cases = @(
	@{{ expected = 'passed'; arguments = @{{ RuntimeCase = 'boundary'; CompileSucceeded = $true; MarkerReady = $true; ReadinessStatus = 'ready'; HasProcessProgress = $true; ControlPassed = $true }} }},
	@{{ expected = 'compile_failure'; arguments = @{{ RuntimeCase = 'control'; CompileSucceeded = $false; MarkerReady = $false; ReadinessStatus = 'not_started'; HasProcessProgress = $false; ControlPassed = $false }} }},
	@{{ expected = 'environment_failure'; arguments = @{{ RuntimeCase = 'control'; CompileSucceeded = $true; MarkerReady = $false; ReadinessStatus = 'timed_out'; HasProcessProgress = $true; ControlPassed = $false }} }},
	@{{ expected = 'boundary_regression'; arguments = @{{ RuntimeCase = 'boundary'; CompileSucceeded = $true; MarkerReady = $false; ReadinessStatus = 'timed_out'; HasProcessProgress = $false; ControlPassed = $true }} }},
	@{{ expected = 'inconclusive_timeout'; arguments = @{{ RuntimeCase = 'boundary'; CompileSucceeded = $true; MarkerReady = $false; ReadinessStatus = 'timed_out'; HasProcessProgress = $true; ControlPassed = $true }} }},
	@{{ expected = 'boundary_regression'; arguments = @{{ RuntimeCase = 'boundary'; CompileSucceeded = $true; MarkerReady = $false; ReadinessStatus = 'process_exited'; HasProcessProgress = $false; ControlPassed = $true }} }}
)
foreach ($case in $cases) {{
	$arguments = $case.arguments
	$actual = Get-PrototypeRuntimeClassification @arguments
	if ($actual -ne $case.expected) {{ throw "Expected $($case.expected), got $actual." }}
}}
"#,
            module.display()
        ),
    )
    .unwrap();

    let output = Command::new("pwsh")
        .args(["-NoLogo", "-NoProfile", "-File"])
        .arg(&harness)
        .output()
        .expect("PowerShell should launch");
    assert!(
        output.status.success(),
        "classification helper failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn public_fixture_evidence_excludes_generated_host_paths() {
    let root = std::env::temp_dir().join(format!(
        "meridian-fixture-evidence-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let module = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/process-readiness.psm1");
    let harness = root.join("harness.ps1");
    std::fs::write(
        &harness,
        format!(
            r#"$ErrorActionPreference = 'Stop'
Import-Module -Force '{}'
$privateRoot = 'C:\Users\Private\fixture'
$inputEvidence = @{{
	layout = 'bucketed'
	declared_leaf_count = 65537
	declared_parent_count = 258
	declared_type_count = 65538
	first_path = '/datum/mlp/p00000'
	boundary_path = '/datum/mlp/p65535'
	last_path = '/datum/mlp/p65536'
	source = "$privateRoot\large_prototypes.dm"
	environment = "$privateRoot\large_prototypes.dme"
}}
$public = ConvertTo-PublicPrototypeFixtureEvidence $inputEvidence
$json = $public | ConvertTo-Json -Compress
if ($json.Contains($privateRoot)) {{ throw 'Public fixture evidence retained a host path.' }}
$keys = @($public.Keys | Sort-Object)
$expected = @('boundary_path', 'declared_leaf_count', 'declared_parent_count', 'declared_type_count', 'first_path', 'last_path', 'layout')
if ([string]::Join(',', $keys) -ne [string]::Join(',', $expected)) {{ throw "Unexpected keys: $($keys -join ',')" }}
"#,
            module.display()
        ),
    )
    .unwrap();

    let output = Command::new("pwsh")
        .args(["-NoLogo", "-NoProfile", "-File"])
        .arg(&harness)
        .output()
        .expect("PowerShell should launch");
    assert!(
        output.status.success(),
        "fixture evidence helper failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(root).unwrap();
}
