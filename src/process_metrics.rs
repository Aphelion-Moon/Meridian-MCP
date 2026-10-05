use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProcessRole {
    MeridianMcp,
    DreamDaemon,
    DreamSeeker,
    Collector,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub started_at_identity: u64,
    pub role: ProcessRole,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryMetricKind {
    WorkingSetBytes,
    PrivateBytes,
    RssBytes,
    VirtualBytes,
}

impl MemoryMetricKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::WorkingSetBytes => "working_set_bytes",
            Self::PrivateBytes => "private_bytes",
            Self::RssBytes => "rss_bytes",
            Self::VirtualBytes => "virtual_bytes",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryUnit {
    Bytes,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct MemorySample {
    pub monotonic_offset_ms: u64,
    pub aligned_tracy_offset: Option<u64>,
    pub metric_kind: MemoryMetricKind,
    pub unit: MemoryUnit,
    pub observed_value: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct RoleMemorySeries {
    pub identity: ProcessIdentity,
    pub operating_system: String,
    pub sampling_interval_ms: u64,
    pub samples: Vec<MemorySample>,
    pub missed_samples: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct MemoryMetricSummary {
    pub sample_count: u64,
    pub median_bytes: u64,
    pub maximum_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct RoleMemorySummary {
    pub identity: ProcessIdentity,
    pub metrics: BTreeMap<String, MemoryMetricSummary>,
    pub missed_samples: u64,
}

pub fn summarize_memory(series: &[RoleMemorySeries]) -> Vec<RoleMemorySummary> {
    series
        .iter()
        .map(|role| {
            let mut values = BTreeMap::<String, Vec<u64>>::new();
            for sample in &role.samples {
                values
                    .entry(sample.metric_kind.as_str().to_owned())
                    .or_default()
                    .push(sample.observed_value);
            }
            let metrics = values
                .into_iter()
                .map(|(kind, mut values)| {
                    values.sort_unstable();
                    let middle = values.len() / 2;
                    let median = if values.len().is_multiple_of(2) {
                        values[middle - 1].saturating_add(values[middle]) / 2
                    } else {
                        values[middle]
                    };
                    (
                        kind,
                        MemoryMetricSummary {
                            sample_count: values.len() as u64,
                            median_bytes: median,
                            maximum_bytes: *values.last().unwrap_or(&0),
                        },
                    )
                })
                .collect();
            RoleMemorySummary {
                identity: role.identity.clone(),
                metrics,
                missed_samples: role.missed_samples,
            }
        })
        .collect()
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessMetricError {
    #[error("process identity is unavailable for pid {0}")]
    IdentityUnavailable(u32),
    #[error("process metrics are unavailable for pid {0}")]
    MetricsUnavailable(u32),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub fn process_identity(
    pid: u32,
    role: ProcessRole,
) -> Result<ProcessIdentity, ProcessMetricError> {
    Ok(ProcessIdentity {
        pid,
        started_at_identity: platform_identity(pid)
            .ok_or(ProcessMetricError::IdentityUnavailable(pid))?,
        role,
    })
}

pub fn sample_process(
    identity: &ProcessIdentity,
    offset_ms: u64,
) -> Result<Vec<MemorySample>, ProcessMetricError> {
    if platform_identity(identity.pid) != Some(identity.started_at_identity) {
        return Err(ProcessMetricError::IdentityUnavailable(identity.pid));
    }
    let values = platform_memory(identity.pid)
        .ok_or(ProcessMetricError::MetricsUnavailable(identity.pid))?;
    Ok(values
        .into_iter()
        .map(|(metric_kind, observed_value)| MemorySample {
            monotonic_offset_ms: offset_ms,
            aligned_tracy_offset: None,
            metric_kind,
            unit: MemoryUnit::Bytes,
            observed_value,
        })
        .collect())
}

#[cfg(windows)]
fn platform_identity(pid: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut creation: FILETIME = std::mem::zeroed();
        let mut exit: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) != 0;
        CloseHandle(handle);
        ok.then(|| (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
    }
}

#[cfg(windows)]
fn platform_memory(pid: u32) -> Option<Vec<(MemoryMetricKind, u64)>> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Memory::{VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_FREE};
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut counters: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        let ok = GetProcessMemoryInfo(
            handle,
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ) != 0;
        let mut system_info: SYSTEM_INFO = std::mem::zeroed();
        GetSystemInfo(&mut system_info);
        let maximum_address = system_info.lpMaximumApplicationAddress as usize;
        let mut address = 0_usize;
        let mut virtual_bytes = 0_u64;
        while address < maximum_address {
            let mut information: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
            let read = VirtualQueryEx(
                handle,
                address as *const core::ffi::c_void,
                &mut information,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            );
            if read == 0 || information.RegionSize == 0 {
                break;
            }
            if information.State != MEM_FREE {
                virtual_bytes = virtual_bytes.saturating_add(information.RegionSize as u64);
            }
            address = (information.BaseAddress as usize).saturating_add(information.RegionSize);
        }
        CloseHandle(handle);
        ok.then(|| {
            vec![
                (
                    MemoryMetricKind::WorkingSetBytes,
                    counters.WorkingSetSize as u64,
                ),
                (MemoryMetricKind::PrivateBytes, counters.PrivateUsage as u64),
                (MemoryMetricKind::VirtualBytes, virtual_bytes),
            ]
        })
    }
}

#[cfg(target_os = "linux")]
fn platform_identity(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let tail = stat.rsplit_once(") ")?.1;
    tail.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(target_os = "linux")]
fn platform_memory(pid: u32) -> Option<Vec<(MemoryMetricKind, u64)>> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let value = |name: &str| -> Option<u64> {
        status
            .lines()
            .find_map(|line| {
                line.strip_prefix(name)?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
            .map(|kb| kb * 1024)
    };
    Some(vec![
        (MemoryMetricKind::RssBytes, value("VmRSS:")?),
        (MemoryMetricKind::VirtualBytes, value("VmSize:")?),
    ])
}

#[cfg(not(any(windows, target_os = "linux")))]
fn platform_identity(_pid: u32) -> Option<u64> {
    None
}

#[cfg(not(any(windows, target_os = "linux")))]
fn platform_memory(_pid: u32) -> Option<Vec<(MemoryMetricKind, u64)>> {
    None
}
