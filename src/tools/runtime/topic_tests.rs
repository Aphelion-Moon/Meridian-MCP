use super::send_topic;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn peer<T: Send + 'static>(
    respond: impl FnOnce(TcpStream) -> T + Send + 'static,
) -> (String, std::thread::JoinHandle<T>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    listener.set_nonblocking(true).unwrap();
    let thread = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "Topic fixture was not contacted");
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("Topic fixture accept failed: {error}"),
            }
        };
        // Windows accepted sockets inherit the listener's nonblocking mode.
        // The peer runs on its own bounded OS thread and must really wait.
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut header = [0; 4];
        stream.read_exact(&mut header).unwrap();
        assert_eq!(&header[..2], &[0, 0x83]);
        let mut body = vec![0; u16::from_be_bytes([header[2], header[3]]) as usize];
        stream.read_exact(&mut body).unwrap();
        assert_eq!(&body, b"\0\0\0\0\0ping\0");
        respond(stream)
    });
    (address, thread)
}

const PONG: &[u8] = b"\x00\x83\x00\x06\x06pong\x00";

#[tokio::test]
async fn topic_invalid_requests_fail_before_runtime_access() {
    use serde_json::json;
    let state = crate::state::ServerState::new();
    let locked = state.runtime().await;
    for args in [
        json!({"topic":"ping","timeout_ms":0}),
        json!({"topic":"ping","timeout_ms":60001}),
        json!({"topic":"ping","timeout_ms":"5000"}),
        json!({"topic":"ping","timeout_ms":null}),
        json!({"topic":false}),
        json!({"topic":"ping\u{0}extra"}),
        json!({"topic":"x".repeat(65530)}),
    ] {
        let result = tokio::time::timeout(Duration::from_millis(100), super::topic(&state, args))
            .await
            .expect("invalid Topic request waited for runtime access")
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let crate::mcp::ToolContent::Text { text } = &result.content[0];
        assert!(text.contains("invalid_input"), "{text}");
    }
    drop(locked);
}

#[tokio::test]
async fn topic_network_wait_yields_to_async_timers() {
    let heartbeat = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&heartbeat);
    let (address, peer) = peer(move |mut stream| {
        std::thread::sleep(Duration::from_millis(400));
        let progressed = observed.load(Ordering::SeqCst);
        stream.write_all(PONG).unwrap();
        progressed
    });
    let timer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        heartbeat.store(true, Ordering::SeqCst);
    });
    let response = send_topic(&address, "?ping", 2000).await.unwrap();
    timer.await.unwrap();
    let progressed = peer.join().unwrap();
    assert_eq!(response, "pong");
    assert!(
        progressed,
        "Topic socket read prevented the async timer from running"
    );
}

#[tokio::test]
async fn topic_deadline_covers_the_entire_trickled_response() {
    let (address, peer) = peer(|mut stream| {
        for byte in PONG {
            std::thread::sleep(Duration::from_millis(30));
            if stream.write_all(&[*byte]).is_err() {
                break;
            }
        }
    });
    let started = Instant::now();
    let response = send_topic(&address, "ping", 100).await;
    let elapsed = started.elapsed();
    peer.join().unwrap();
    assert!(
        response.is_err(),
        "Topic accepted a response beyond its total deadline: elapsed={elapsed:?}, response={response:?}"
    );
    assert!(response.unwrap_err().to_string().contains("timed out"));
}

#[tokio::test]
async fn topic_cancellation_closes_the_pending_socket() {
    let (started, ready) = std::sync::mpsc::channel();
    let (address, peer) = peer(move |mut stream| {
        started.send(()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        stream.read(&mut [0])
    });
    let request = tokio::spawn(async move { send_topic(&address, "ping", 2000).await });
    let abort = request.abort_handle();
    let canceller = std::thread::spawn(move || {
        ready.recv_timeout(Duration::from_secs(3)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        abort.abort();
    });
    let outcome = request.await;
    canceller.join().unwrap();
    let closed = peer.join().unwrap();
    assert!(
        matches!(closed, Ok(0)),
        "cancelled Topic did not close its socket: peer_read={closed:?}, request={outcome:?}"
    );
    assert!(outcome.unwrap_err().is_cancelled());
}
