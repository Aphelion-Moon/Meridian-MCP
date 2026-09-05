use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::Child;

pub const AUXTOOLS_VERSION: &str = "v2.3.7";
pub const AUXTOOLS_SHA256: &str =
    "b188999ac58a0e0171b015c39a403ab7da2f37ddb8ac3817a078f5bce02a8be7";
pub const AUXTOOLS_RELEASE_URL: &str =
    "https://github.com/willox/auxtools/releases/download/v2.3.7/debug_server.dll";

#[derive(Clone, Debug)]
pub struct DebuggerInstallation {
    pub dreamseeker: PathBuf,
    pub dreamdaemon: PathBuf,
    pub debug_server_dll: PathBuf,
    pub dll_sha256: String,
}
#[derive(Debug, thiserror::Error)]
pub enum DebuggerPolicyError {
    #[error("auxtools debugger is Windows-only")]
    Platform,
    #[error("auxtools requires exactly one allowlisted dm.exe")]
    Compiler,
    #[error("DreamSeeker sibling not found")]
    DreamSeeker,
    #[error("DreamDaemon sibling not found")]
    DreamDaemon,
    #[error("debug_server.dll not found at the fixed helper location")]
    Dll,
    #[error("debug_server.dll checksum mismatch")]
    Checksum,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
pub fn validate_installation(
    compilers: &[PathBuf],
) -> Result<DebuggerInstallation, DebuggerPolicyError> {
    if !cfg!(windows) {
        return Err(DebuggerPolicyError::Platform);
    }
    if compilers.len() != 1
        || !compilers[0]
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("dm.exe"))
    {
        return Err(DebuggerPolicyError::Compiler);
    }
    let dreamseeker = compilers[0]
        .parent()
        .ok_or(DebuggerPolicyError::DreamSeeker)?
        .join("dreamseeker.exe")
        .canonicalize()
        .map_err(|_| DebuggerPolicyError::DreamSeeker)?;
    let dreamdaemon = compilers[0]
        .parent()
        .ok_or(DebuggerPolicyError::DreamDaemon)?
        .join("dreamdaemon.exe")
        .canonicalize()
        .map_err(|_| DebuggerPolicyError::DreamDaemon)?;
    let root = std::env::current_exe()?
        .parent()
        .ok_or(DebuggerPolicyError::Dll)?
        .to_owned();
    let dll = root
        .join("helpers/auxtools/v2.3.7/debug_server.dll")
        .canonicalize()
        .map_err(|_| DebuggerPolicyError::Dll)?;
    let hash = format!("{:x}", Sha256::digest(std::fs::read(&dll)?));
    if hash != AUXTOOLS_SHA256 {
        return Err(DebuggerPolicyError::Checksum);
    }
    Ok(DebuggerInstallation {
        dreamseeker,
        dreamdaemon,
        debug_server_dll: dll,
        dll_sha256: hash,
    })
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum AuxRequest {
    Disconnect,
    Configured,
    StdDef,
    Eval {
        frame_id: Option<u32>,
        command: String,
        context: Option<String>,
    },
    CurrentInstruction {
        frame_id: u32,
    },
    BreakpointSet {
        instruction: InstructionRef,
        condition: Option<String>,
    },
    BreakpointUnset {
        instruction: InstructionRef,
    },
    CatchRuntimes {
        should_catch: bool,
    },
    LineNumber {
        proc: ProcRef,
        offset: u32,
    },
    Offset {
        proc: ProcRef,
        line: u32,
    },
    Stacks,
    StackFrames {
        stack_id: u32,
        start_frame: Option<u32>,
        count: Option<u32>,
    },
    Scopes {
        frame_id: u32,
    },
    Variables {
        vars: VariablesRef,
    },
    Continue {
        kind: ContinueKind,
    },
    Pause,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum AuxResponse {
    Ack,
    StdDef(Option<String>),
    Eval(EvalResponse),
    CurrentInstruction(Option<InstructionRef>),
    BreakpointSet {
        result: BreakpointSetResult,
    },
    BreakpointUnset {
        success: bool,
    },
    LineNumber {
        line: Option<u32>,
    },
    Offset {
        offset: Option<u32>,
    },
    Stacks {
        stacks: Vec<Stack>,
    },
    StackFrames {
        frames: Vec<StackFrame>,
        total_count: u32,
    },
    Scopes {
        arguments: Option<VariablesRef>,
        locals: Option<VariablesRef>,
        globals: Option<VariablesRef>,
    },
    Variables {
        vars: Vec<Variable>,
    },
    Disconnect,
    Notification {
        message: String,
    },
    BreakpointHit {
        reason: BreakpointReason,
    },
}
#[derive(Serialize, Deserialize, Debug, Hash, PartialEq, Eq, Clone)]
pub struct ProcRef {
    pub path: String,
    pub override_id: u32,
}
#[derive(Serialize, Deserialize, Debug, Hash, PartialEq, Eq, Clone)]
pub struct InstructionRef {
    pub proc: ProcRef,
    pub offset: u32,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum BreakpointReason {
    Breakpoint,
    Step,
    Pause,
    Runtime(String),
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ContinueKind {
    Continue,
    StepOver { stack_id: u32 },
    StepInto { stack_id: u32 },
    StepOut { stack_id: u32 },
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Stack {
    pub id: u32,
    pub name: String,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StackFrame {
    pub id: u32,
    pub instruction: InstructionRef,
    pub line: Option<u32>,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum BreakpointSetResult {
    Success { line: Option<u32> },
    Failed,
}
#[derive(Clone, Hash, Eq, PartialEq, Serialize, Deserialize, Debug)]
pub struct VariablesRef(pub i32);
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub variables: Option<VariablesRef>,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EvalResponse {
    pub value: String,
    pub variables: Option<VariablesRef>,
}

#[derive(Debug, thiserror::Error)]
pub enum AuxProtocolError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Codec(#[from] bincode::Error),
    #[error("auxtools message length {0} is invalid")]
    Length(u32),
    #[error("auxtools response timed out; stop and relaunch the debugger")]
    Timeout,
    #[error("auxtools disconnected; stop and relaunch the debugger")]
    Disconnected,
}
pub struct AuxConnection {
    stream: TcpStream,
    max_message_bytes: usize,
    response_timeout: Duration,
    events: VecDeque<AuxResponse>,
    failed: bool,
    incoming_header: [u8; 4],
    header_read: usize,
    incoming: Vec<u8>,
    payload_read: usize,
}
impl AuxConnection {
    pub fn new(stream: TcpStream, max_message_bytes: usize, response_timeout: Duration) -> Self {
        Self {
            stream,
            max_message_bytes,
            response_timeout,
            events: VecDeque::new(),
            failed: false,
            incoming_header: [0; 4],
            header_read: 0,
            incoming: Vec::new(),
            payload_read: 0,
        }
    }
    pub async fn request(&mut self, request: AuxRequest) -> Result<AuxResponse, AuxProtocolError> {
        if self.failed {
            return Err(AuxProtocolError::Disconnected);
        }
        // The wire has no request IDs. Any timeout or cancelled request makes
        // subsequent replies ambiguous, even when a complete frame arrives later.
        self.failed = true;
        let result = tokio::time::timeout(self.response_timeout, async {
            self.write_request(request).await?;
            loop {
                match self.read_response().await? {
                    response @ (AuxResponse::Notification { .. }
                    | AuxResponse::BreakpointHit { .. }) => self.events.push_back(response),
                    AuxResponse::Disconnect => return Err(AuxProtocolError::Disconnected),
                    response => return Ok(response),
                }
            }
        })
        .await
        .unwrap_or(Err(AuxProtocolError::Timeout));
        self.failed = result.is_err();
        if self.failed {
            let _ = self.stream.shutdown().await;
        }
        result
    }
    pub async fn send(&mut self, request: AuxRequest) -> Result<(), AuxProtocolError> {
        if self.failed {
            return Err(AuxProtocolError::Disconnected);
        }
        self.failed = true;
        let result = tokio::time::timeout(self.response_timeout, self.write_request(request))
            .await
            .unwrap_or(Err(AuxProtocolError::Timeout));
        self.failed = result.is_err();
        result
    }
    async fn write_request(&mut self, request: AuxRequest) -> Result<(), AuxProtocolError> {
        let payload = bincode::serialize(&request)?;
        if payload.is_empty() || payload.len() > self.max_message_bytes {
            return Err(AuxProtocolError::Length(payload.len() as u32));
        }
        self.stream.write_u32_le(payload.len() as u32).await?;
        self.stream.write_all(&payload).await?;
        Ok(())
    }
    async fn read_response(&mut self) -> Result<AuxResponse, AuxProtocolError> {
        // read() is cancellation-safe; retain progress before the next await so
        // an ordinary event-wait timeout cannot discard a partial wire frame.
        while self.header_read < 4 {
            let read = self
                .stream
                .read(&mut self.incoming_header[self.header_read..])
                .await?;
            if read == 0 {
                return Err(AuxProtocolError::Disconnected);
            }
            self.header_read += read;
        }
        let len = u32::from_le_bytes(self.incoming_header);
        if len == 0 || len as usize > self.max_message_bytes {
            return Err(AuxProtocolError::Length(len));
        }
        self.incoming.resize(len as usize, 0);
        while self.payload_read < len as usize {
            let read = self
                .stream
                .read(&mut self.incoming[self.payload_read..])
                .await?;
            if read == 0 {
                return Err(AuxProtocolError::Disconnected);
            }
            self.payload_read += read;
        }
        let response = bincode::deserialize(&self.incoming)?;
        self.header_read = 0;
        self.payload_read = 0;
        self.incoming.clear();
        Ok(response)
    }
    pub fn pop_event(&mut self) -> Option<AuxResponse> {
        self.events.pop_front()
    }
    pub async fn next_event(&mut self, timeout: Duration) -> Result<AuxResponse, AuxProtocolError> {
        if self.failed {
            return Err(AuxProtocolError::Disconnected);
        }
        if let Some(event) = self.events.pop_front() {
            return Ok(event);
        }
        match tokio::time::timeout(timeout, self.read_response()).await {
            Err(_) => Err(AuxProtocolError::Timeout),
            Ok(result) => {
                self.failed = result.is_err();
                result
            }
        }
    }
    pub async fn disconnect(&mut self) -> Result<(), AuxProtocolError> {
        // Upstream's reader exits without acknowledging Disconnect. The owned
        // process will be terminated by the caller, so only bound the send.
        let _ =
            tokio::time::timeout(Duration::from_secs(1), self.send(AuxRequest::Disconnect)).await;
        self.failed = true;
        self.stream.shutdown().await?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DebuggerLifecycle {
    Running,
    Failed,
}

pub struct DebuggerSession {
    pub lifecycle: DebuggerLifecycle,
    pub process: Child,
    pub connection: AuxConnection,
    pub port: u16,
    pub dmb_path: PathBuf,
    pub stddef_source: Option<String>,
    pub state_generation: u64,
    pub event_sequence: u64,
    pub last_exception: Option<String>,
    pub active_breakpoints: HashSet<InstructionRef>,
    pub events: VecDeque<DebuggerEventRecord>,
    pub dropped_events: u64,
    pub launch_provenance: crate::LaunchProvenance,
    pub memory_helper_sha256: Option<String>,
    pub(crate) containment: crate::process::ProcessContainment,
}

#[derive(Clone, Debug, Serialize)]
pub struct DebuggerEventRecord {
    pub sequence: u64,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl DebuggerSession {
    pub async fn stop(mut self) -> Result<(), AuxProtocolError> {
        let _ = self.connection.disconnect().await;
        let _ = self.containment.terminate(1);
        let _ = self.process.kill().await;
        let _ = self.process.wait().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn a_timeout_prevents_reusing_a_late_reply_for_another_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (release, ready) = tokio::sync::oneshot::channel();
        let (sent, received) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let len = stream.read_u32_le().await.unwrap();
            let mut request = vec![0; len as usize];
            stream.read_exact(&mut request).await.unwrap();
            ready.await.unwrap();
            let payload = bincode::serialize(&AuxResponse::Ack).unwrap();
            let _ = stream.write_u32_le(payload.len() as u32).await;
            let _ = stream.write_all(&payload).await;
            sent.send(()).unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        });
        let stream = TcpStream::connect(address).await.unwrap();
        let mut connection = AuxConnection::new(stream, 1024, Duration::from_millis(20));
        assert!(matches!(
            connection.request(AuxRequest::Configured).await,
            Err(AuxProtocolError::Timeout)
        ));
        release.send(()).unwrap();
        received.await.unwrap();
        assert!(matches!(
            connection.request(AuxRequest::Pause).await,
            Err(AuxProtocolError::Disconnected)
        ));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn disconnect_does_not_wait_for_an_ack_the_upstream_never_sends() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let len = stream.read_u32_le().await.unwrap();
            let mut request = vec![0; len as usize];
            stream.read_exact(&mut request).await.unwrap();
            assert!(matches!(
                bincode::deserialize::<AuxRequest>(&request).unwrap(),
                AuxRequest::Disconnect
            ));
            let mut byte = [0];
            assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
        });
        let stream = TcpStream::connect(address).await.unwrap();
        let mut connection = AuxConnection::new(stream, 1024, Duration::from_secs(30));
        tokio::time::timeout(Duration::from_millis(500), connection.disconnect())
            .await
            .expect("Disconnect must not wait for a response")
            .unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn event_wait_timeout_retains_a_partial_frame() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (sent, received) = tokio::sync::oneshot::channel();
        let (release, ready) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let payload = bincode::serialize(&AuxResponse::Notification {
                message: "split event".into(),
            })
            .unwrap();
            let header = (payload.len() as u32).to_le_bytes();
            stream.write_all(&header[..2]).await.unwrap();
            sent.send(()).unwrap();
            ready.await.unwrap();
            stream.write_all(&header[2..]).await.unwrap();
            stream.write_all(&payload).await.unwrap();
        });
        let stream = TcpStream::connect(address).await.unwrap();
        let mut connection = AuxConnection::new(stream, 1024, Duration::from_secs(1));
        received.await.unwrap();
        assert!(matches!(
            connection.next_event(Duration::from_millis(20)).await,
            Err(AuxProtocolError::Timeout)
        ));
        release.send(()).unwrap();
        assert!(
            matches!(connection.next_event(Duration::from_secs(1)).await.unwrap(), AuxResponse::Notification { message } if message == "split event")
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn bounded_protocol_routes_interleaved_events() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let len = stream.read_u32_le().await.unwrap();
            let mut request = vec![0; len as usize];
            stream.read_exact(&mut request).await.unwrap();
            assert!(matches!(
                bincode::deserialize::<AuxRequest>(&request).unwrap(),
                AuxRequest::Stacks
            ));
            for response in [
                AuxResponse::Notification {
                    message: "technical fixture".into(),
                },
                AuxResponse::Stacks {
                    stacks: vec![Stack {
                        id: 1,
                        name: "main".into(),
                    }],
                },
            ] {
                let payload = bincode::serialize(&response).unwrap();
                stream.write_u32_le(payload.len() as u32).await.unwrap();
                stream.write_all(&payload).await.unwrap();
            }
        });
        let stream = TcpStream::connect(address).await.unwrap();
        let mut connection = AuxConnection::new(stream, 8 * 1024 * 1024, Duration::from_secs(1));
        let response = connection.request(AuxRequest::Stacks).await.unwrap();
        assert!(matches!(response, AuxResponse::Stacks { .. }));
        assert!(matches!(
            connection.pop_event(),
            Some(AuxResponse::Notification { .. })
        ));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn oversized_incoming_length_is_rejected_before_allocation() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let len = stream.read_u32_le().await.unwrap();
            let mut request = vec![0; len as usize];
            stream.read_exact(&mut request).await.unwrap();
            stream.write_u32_le(1025).await.unwrap();
        });
        let stream = TcpStream::connect(address).await.unwrap();
        let mut connection = AuxConnection::new(stream, 1024, Duration::from_secs(1));
        assert!(matches!(
            connection.request(AuxRequest::Stacks).await,
            Err(AuxProtocolError::Length(1025))
        ));
    }

    #[tokio::test]
    async fn send_only_request_does_not_wait_for_a_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let len = stream.read_u32_le().await.unwrap();
            let mut request = vec![0; len as usize];
            stream.read_exact(&mut request).await.unwrap();
            assert!(matches!(
                bincode::deserialize::<AuxRequest>(&request).unwrap(),
                AuxRequest::CatchRuntimes { should_catch: true }
            ));
        });
        let stream = TcpStream::connect(address).await.unwrap();
        let mut connection = AuxConnection::new(stream, 1024, Duration::from_millis(10));
        connection
            .send(AuxRequest::CatchRuntimes { should_catch: true })
            .await
            .unwrap();
        server.await.unwrap();
    }
}
