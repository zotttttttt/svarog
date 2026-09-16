use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut expected = None;
    loop {
        let read = stream.read(&mut buffer).unwrap_or(0);
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        if expected.is_none() {
            if let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                expected = Some(header_end + 4 + content_length);
            }
        }
        if expected.is_some_and(|length| request.len() >= length) {
            break;
        }
    }
    request
}

fn spawn_fake_anthropic() -> (SocketAddr, Arc<AtomicBool>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let handle = thread::spawn(move || {
        let body = r#"{"id":"msg_test","type":"message","role":"assistant","model":"claude-sonnet-4-5-20250929","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}"#;
        while !thread_stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = read_http_request(&mut stream);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });
    (address, stop, handle)
}

fn spawn_collector() -> (
    SocketAddr,
    Arc<AtomicBool>,
    mpsc::Receiver<String>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        while !thread_stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let request = read_http_request(&mut stream);
                    if let Some(body_start) =
                        request.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        let body = String::from_utf8_lossy(&request[body_start + 4..]).to_string();
                        let _ = sender.send(body);
                    }
                    let _ = stream.write_all(
                        b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });
    (address, stop, receiver, handle)
}

#[test]
#[ignore = "requires an installed Claude Code CLI"]
fn real_claude_cli_loads_and_fires_svarog_hooks_without_external_network() {
    if Command::new("claude").arg("--version").output().is_err() {
        eprintln!("Claude Code is not installed; skipping smoke test");
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let svarog_home = root.path().join("svarog");
    let claude_home = root.path().join("claude");
    fs::create_dir_all(&svarog_home).unwrap();
    fs::write(
        svarog_home.join("collector.token"),
        format!("{}\n", "a".repeat(64)),
    )
    .unwrap();
    let (collector_addr, collector_stop, receiver, collector_handle) = spawn_collector();
    let (api_addr, api_stop, api_handle) = spawn_fake_anthropic();

    let install = Command::new(env!("CARGO_BIN_EXE_svarog"))
        .args(["hook", "claude", "--install"])
        .env("SVAROG_HOME", &svarog_home)
        .env("CLAUDE_CONFIG_DIR", &claude_home)
        .env("SVAROG_DAEMON_ADDR", collector_addr.to_string())
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "hook install failed: {}",
        String::from_utf8_lossy(&install.stderr)
    );

    let output = Command::new("claude")
        .args([
            "-p",
            "--tools",
            "",
            "--setting-sources",
            "user",
            "--no-session-persistence",
            "reply with ok",
        ])
        .env("CLAUDE_CONFIG_DIR", &claude_home)
        .env("ANTHROPIC_API_KEY", "smoke-test-key")
        .env("ANTHROPIC_BASE_URL", format!("http://{api_addr}"))
        .env("DISABLE_AUTOUPDATER", "1")
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .output()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    while Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => events.push(event),
            Err(mpsc::RecvTimeoutError::Timeout) if events.len() < 4 => continue,
            Err(_) => break,
        }
        if events.len() >= 4 {
            break;
        }
    }
    collector_stop.store(true, Ordering::Relaxed);
    api_stop.store(true, Ordering::Relaxed);
    collector_handle.join().unwrap();
    api_handle.join().unwrap();

    assert!(
        output.status.success(),
        "Claude smoke run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    for name in ["SessionStart", "UserPromptSubmit", "Stop", "SessionEnd"] {
        assert!(
            events.iter().any(|event| event.contains(name)),
            "missing {name}; received {events:?}"
        );
    }
    assert!(events.iter().all(|event| !event.contains("reply with ok")));
}
