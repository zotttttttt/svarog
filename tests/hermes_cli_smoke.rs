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
#[ignore = "requires an installed Hermes Agent CLI"]
fn real_hermes_cli_loads_and_fires_svarog_hooks_without_external_network() {
    if Command::new("hermes").arg("--version").output().is_err() {
        eprintln!("Hermes Agent is not installed; skipping smoke test");
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let svarog_home = root.path().join("svarog");
    let hermes_home = root.path().join("hermes");
    fs::create_dir_all(&svarog_home).unwrap();
    fs::create_dir_all(&hermes_home).unwrap();
    fs::write(
        hermes_home.join("config.yaml"),
        "# existing Hermes config\nmodel:\n  default: test-model\n",
    )
    .unwrap();
    fs::write(
        svarog_home.join("collector.token"),
        format!("{}\n", "a".repeat(64)),
    )
    .unwrap();
    let payload_file = root.path().join("payload.json");
    fs::write(
        &payload_file,
        r#"{"session_id":"session-1","turn_id":"turn-1","user_message":"private prompt","conversation_history":["private history"],"assistant_response":"private response","completed":true}"#,
    )
    .unwrap();
    let (collector_addr, stop, receiver, handle) = spawn_collector();

    let install = Command::new(env!("CARGO_BIN_EXE_svarog"))
        .args(["hook", "hermes", "--install"])
        .env("SVAROG_HOME", &svarog_home)
        .env("HERMES_HOME", &hermes_home)
        .env("SVAROG_DAEMON_ADDR", collector_addr.to_string())
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "hook install failed: {}",
        String::from_utf8_lossy(&install.stderr)
    );
    let installed_config = fs::read(hermes_home.join("config.yaml")).unwrap();
    assert!(String::from_utf8_lossy(&installed_config).contains("# existing Hermes config"));

    let reinstall = Command::new(env!("CARGO_BIN_EXE_svarog"))
        .args(["hook", "hermes", "--install"])
        .env("SVAROG_HOME", &svarog_home)
        .env("HERMES_HOME", &hermes_home)
        .env("SVAROG_DAEMON_ADDR", collector_addr.to_string())
        .output()
        .unwrap();
    assert!(
        reinstall.status.success(),
        "hook reinstall failed: {}",
        String::from_utf8_lossy(&reinstall.stderr)
    );
    assert_eq!(
        fs::read(hermes_home.join("config.yaml")).unwrap(),
        installed_config
    );

    for args in [vec!["hooks", "list"], vec!["hooks", "doctor"]] {
        let output = Command::new("hermes")
            .args(args)
            .env("HERMES_HOME", &hermes_home)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Hermes hook inspection failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    for event in [
        "on_session_start",
        "pre_llm_call",
        "on_session_end",
        "on_session_finalize",
    ] {
        let output = Command::new("hermes")
            .args(["hooks", "test", event, "--payload-file"])
            .arg(&payload_file)
            .env("HERMES_HOME", &hermes_home)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Hermes {event} test failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    while Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => events.push(event),
            Err(mpsc::RecvTimeoutError::Timeout) if events.len() < 4 => continue,
            Err(_) => break,
        }
        if events.len() >= 4
            && ["SessionStart", "UserPromptSubmit", "Stop", "SessionEnd"]
                .iter()
                .all(|name| events.iter().any(|event| event.contains(name)))
        {
            break;
        }
    }
    stop.store(true, Ordering::Relaxed);
    handle.join().unwrap();

    for name in ["SessionStart", "UserPromptSubmit", "Stop", "SessionEnd"] {
        assert!(
            events.iter().any(|event| event.contains(name)),
            "missing {name}; received {events:?}"
        );
    }
    for private in ["private prompt", "private history", "private response"] {
        assert!(events.iter().all(|event| !event.contains(private)));
    }
}
