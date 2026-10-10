use crate::process_metrics::ProcessIdentity;
use serde::Serialize;
#[cfg(windows)]
use std::collections::HashMap;

pub const MAX_NETWORK_OBSERVATIONS: usize = 256;

#[derive(
    Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum EndpointProtocol {
    Tcp,
    Udp,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct EndpointObservation {
    pub protocol: EndpointProtocol,
    pub process_id: u32,
    pub local_endpoint: String,
    pub remote_endpoint: Option<String>,
    pub first_seen_ms: u128,
    pub last_seen_ms: u128,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct NetworkAuditReport {
    pub requested: bool,
    pub available: bool,
    pub capture_complete: bool,
    pub truncated: bool,
    pub observations: Vec<EndpointObservation>,
    pub warning: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct OwnedEndpointObservation {
    pub owner: ProcessIdentity,
    pub protocol: EndpointProtocol,
    pub local_endpoint: String,
    pub remote_endpoint: Option<String>,
    pub first_seen_ms: u128,
    pub last_seen_ms: u128,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct NetworkEvidence {
    pub mode: String,
    pub network_isolation_confirmed: bool,
    pub capture_complete: bool,
    pub listener_verified: bool,
    pub collector_connection_verified: bool,
    pub owned_loopback_endpoints: Vec<OwnedEndpointObservation>,
    pub observation_failures: Vec<String>,
}

pub fn tracy_network_evidence(
    report: NetworkAuditReport,
    profiler_port: u16,
    identities: &[ProcessIdentity],
    listener_verified: bool,
    collector_connection_verified: bool,
) -> NetworkEvidence {
    let mut observation_failures = report.warning.into_iter().collect::<Vec<_>>();
    if report.truncated {
        observation_failures.push("network_observations_truncated".into());
    }
    let owned_loopback_endpoints = report
        .observations
        .into_iter()
        .filter_map(|observation| {
            let owner = identities
                .iter()
                .find(|identity| identity.pid == observation.process_id)?
                .clone();
            let relevant = endpoint_is_loopback_port(&observation.local_endpoint, profiler_port)
                || observation
                    .remote_endpoint
                    .as_deref()
                    .is_some_and(|endpoint| endpoint_is_loopback_port(endpoint, profiler_port));
            relevant.then_some(OwnedEndpointObservation {
                owner,
                protocol: observation.protocol,
                local_endpoint: observation.local_endpoint,
                remote_endpoint: observation.remote_endpoint,
                first_seen_ms: observation.first_seen_ms,
                last_seen_ms: observation.last_seen_ms,
            })
        })
        .collect();
    NetworkEvidence {
        mode: "best_effort_owned_loopback".into(),
        network_isolation_confirmed: false,
        capture_complete: false,
        listener_verified,
        collector_connection_verified,
        owned_loopback_endpoints,
        observation_failures,
    }
}

fn endpoint_is_loopback_port(endpoint: &str, port: u16) -> bool {
    let port = format!(":{port}");
    endpoint.ends_with(&port) && (endpoint.starts_with("127.") || endpoint.starts_with("[::1]"))
}

#[cfg(windows)]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ObservationKey {
    protocol: EndpointProtocol,
    process_id: u32,
    local_endpoint: String,
    remote_endpoint: Option<String>,
}

pub(crate) struct NetworkAuditCollector {
    report: NetworkAuditReport,
    #[cfg(windows)]
    indices: HashMap<ObservationKey, usize>,
}

impl NetworkAuditCollector {
    pub(crate) fn new(requested: bool) -> Self {
        let available = requested && cfg!(windows);
        Self {
            report: NetworkAuditReport {
                requested,
                available,
                capture_complete: false,
                truncated: false,
                observations: Vec::new(),
                warning: (requested && !cfg!(windows))
                    .then(|| "network_audit_unavailable".to_string()),
            },
            #[cfg(windows)]
            indices: HashMap::new(),
        }
    }

    #[cfg(windows)]
    pub(crate) fn sample(&mut self, process_ids: &[u32], elapsed_ms: u128) {
        if !self.report.requested || !self.report.available || process_ids.is_empty() {
            return;
        }
        match windows::sample(process_ids) {
            Ok(observations) => {
                for (protocol, process_id, local_endpoint, remote_endpoint) in observations {
                    self.record(
                        protocol,
                        process_id,
                        local_endpoint,
                        remote_endpoint,
                        elapsed_ms,
                    );
                }
            }
            Err(error) => {
                self.report.available = false;
                self.report.warning = Some(format!("network_audit_unavailable: {error}"));
            }
        }
    }

    #[cfg(not(windows))]
    pub(crate) fn sample(&mut self, _process_ids: &[u32], _elapsed_ms: u128) {}

    #[cfg(windows)]
    fn record(
        &mut self,
        protocol: EndpointProtocol,
        process_id: u32,
        local_endpoint: String,
        remote_endpoint: Option<String>,
        elapsed_ms: u128,
    ) {
        let key = ObservationKey {
            protocol,
            process_id,
            local_endpoint: local_endpoint.clone(),
            remote_endpoint: remote_endpoint.clone(),
        };
        if let Some(index) = self.indices.get(&key).copied() {
            self.report.observations[index].last_seen_ms = elapsed_ms;
            return;
        }
        if self.report.observations.len() >= MAX_NETWORK_OBSERVATIONS {
            self.report.truncated = true;
            return;
        }
        let index = self.report.observations.len();
        self.report.observations.push(EndpointObservation {
            protocol,
            process_id,
            local_endpoint,
            remote_endpoint,
            first_seen_ms: elapsed_ms,
            last_seen_ms: elapsed_ms,
        });
        self.indices.insert(key, index);
    }

    pub(crate) fn finish(self) -> NetworkAuditReport {
        self.report
    }
}

#[cfg(windows)]
mod windows {
    use super::EndpointProtocol;
    use std::collections::HashSet;
    use std::ffi::c_void;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::ptr;
    use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, GetExtendedUdpTable, MIB_TCP6TABLE_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
        MIB_UDP6TABLE_OWNER_PID, MIB_UDPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
        UDP_TABLE_OWNER_PID,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

    type RawObservation = (EndpointProtocol, u32, String, Option<String>);

    pub(super) fn sample(process_ids: &[u32]) -> Result<Vec<RawObservation>, String> {
        let process_ids = process_ids.iter().copied().collect::<HashSet<_>>();
        let mut output = Vec::new();
        unsafe {
            let tcp4 = tcp_table(AF_INET as u32)?;
            let table = &*(tcp4.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>());
            for row in std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize)
            {
                if process_ids.contains(&row.dwOwningPid) {
                    output.push((
                        EndpointProtocol::Tcp,
                        row.dwOwningPid,
                        ipv4_endpoint(row.dwLocalAddr, row.dwLocalPort),
                        remote_ipv4_endpoint(row.dwRemoteAddr, row.dwRemotePort),
                    ));
                }
            }

            let tcp6 = tcp_table(AF_INET6 as u32)?;
            let table = &*(tcp6.as_ptr().cast::<MIB_TCP6TABLE_OWNER_PID>());
            for row in std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize)
            {
                if process_ids.contains(&row.dwOwningPid) {
                    output.push((
                        EndpointProtocol::Tcp,
                        row.dwOwningPid,
                        ipv6_endpoint(row.ucLocalAddr, row.dwLocalPort),
                        remote_ipv6_endpoint(row.ucRemoteAddr, row.dwRemotePort),
                    ));
                }
            }

            let udp4 = udp_table(AF_INET as u32)?;
            let table = &*(udp4.as_ptr().cast::<MIB_UDPTABLE_OWNER_PID>());
            for row in std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize)
            {
                if process_ids.contains(&row.dwOwningPid) {
                    output.push((
                        EndpointProtocol::Udp,
                        row.dwOwningPid,
                        ipv4_endpoint(row.dwLocalAddr, row.dwLocalPort),
                        None,
                    ));
                }
            }

            let udp6 = udp_table(AF_INET6 as u32)?;
            let table = &*(udp6.as_ptr().cast::<MIB_UDP6TABLE_OWNER_PID>());
            for row in std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize)
            {
                if process_ids.contains(&row.dwOwningPid) {
                    output.push((
                        EndpointProtocol::Udp,
                        row.dwOwningPid,
                        ipv6_endpoint(row.ucLocalAddr, row.dwLocalPort),
                        None,
                    ));
                }
            }
        }
        Ok(output)
    }

    unsafe fn tcp_table(address_family: u32) -> Result<Vec<u8>, String> {
        query_table(|buffer, size| {
            GetExtendedTcpTable(buffer, size, 0, address_family, TCP_TABLE_OWNER_PID_ALL, 0)
        })
    }

    unsafe fn udp_table(address_family: u32) -> Result<Vec<u8>, String> {
        query_table(|buffer, size| {
            GetExtendedUdpTable(buffer, size, 0, address_family, UDP_TABLE_OWNER_PID, 0)
        })
    }

    unsafe fn query_table(query: impl Fn(*mut c_void, *mut u32) -> u32) -> Result<Vec<u8>, String> {
        let mut size = 0_u32;
        let initial = query(ptr::null_mut(), &mut size);
        if initial != ERROR_INSUFFICIENT_BUFFER && initial != NO_ERROR {
            return Err(format!("table sizing failed with Windows error {initial}"));
        }
        let mut buffer = vec![0_u8; size as usize];
        let result = query(buffer.as_mut_ptr().cast(), &mut size);
        if result != NO_ERROR {
            return Err(format!("table query failed with Windows error {result}"));
        }
        Ok(buffer)
    }

    fn port(value: u32) -> u16 {
        u16::from_be(value as u16)
    }

    fn ipv4_endpoint(address: u32, raw_port: u32) -> String {
        format!(
            "{}:{}",
            Ipv4Addr::from(address.to_ne_bytes()),
            port(raw_port)
        )
    }

    fn remote_ipv4_endpoint(address: u32, raw_port: u32) -> Option<String> {
        (raw_port != 0).then(|| ipv4_endpoint(address, raw_port))
    }

    fn ipv6_endpoint(address: [u8; 16], raw_port: u32) -> String {
        format!("[{}]:{}", Ipv6Addr::from(address), port(raw_port))
    }

    fn remote_ipv6_endpoint(address: [u8; 16], raw_port: u32) -> Option<String> {
        (raw_port != 0).then(|| ipv6_endpoint(address, raw_port))
    }
}
