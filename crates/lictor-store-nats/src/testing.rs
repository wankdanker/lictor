//! Throwaway `nats-server` processes for tests.
//!
//! [`NatsServer::start`] runs `nats-server -js` on a free port with a temporary
//! storage directory and stops it on drop. The binary is found via `$NATS_SERVER_BIN`
//! or `PATH`. When it is missing, `start` returns `None` so tests can skip, unless
//! `LICTOR_REQUIRE_NATS=1` is set (as in CI), in which case it panics.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A running `nats-server`, killed when dropped.
pub struct NatsServer {
    port: u16,
    dir: tempfile::TempDir,
    child: Option<Child>,
}

fn binary() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NATS_SERVER_BIN") {
        return Some(PathBuf::from(p));
    }
    let exe = if cfg!(windows) { "nats-server.exe" } else { "nats-server" };
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(exe)).find(|p| p.is_file()))
}

fn required() -> bool {
    std::env::var("LICTOR_REQUIRE_NATS").is_ok_and(|v| v == "1" || v == "true")
}

impl NatsServer {
    /// Start a server, or `None` if `nats-server` is not installed (and not required).
    pub fn start() -> Option<NatsServer> {
        let Some(_) = binary() else {
            if required() {
                panic!("LICTOR_REQUIRE_NATS is set but nats-server was not found on PATH or in NATS_SERVER_BIN");
            }
            return None;
        };
        let mut s = NatsServer { port: 0, dir: tempfile::tempdir().expect("tempdir"), child: None };
        s.spawn();
        Some(s)
    }

    /// Start the process. The first start lets the server pick a free port (`-p -1`) and
    /// reads it back from the ports file, so parallel tests never race for a port.
    fn spawn(&mut self) {
        let ports_dir = self.dir.path().join("ports");
        let _ = std::fs::remove_dir_all(&ports_dir);
        std::fs::create_dir_all(&ports_dir).expect("ports dir");
        let port_arg = if self.port == 0 { "-1".to_string() } else { self.port.to_string() };
        let child = Command::new(binary().expect("nats-server binary"))
            .args(["-js", "-a", "127.0.0.1", "-p", &port_arg, "--ports_file_dir"])
            .arg(&ports_dir)
            .arg("-sd")
            .arg(self.dir.path().join("js"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn nats-server");
        self.child = Some(child);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(port) = read_port(&ports_dir) {
                self.port = port;
                if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    return;
                }
            }
            assert!(Instant::now() < deadline, "nats-server did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// `nats://127.0.0.1:<port>`.
    pub fn url(&self) -> String {
        format!("nats://127.0.0.1:{}", self.port)
    }

    /// The port the server listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Kill the server (simulating an outage). Data on disk is kept.
    pub fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// Start the server again on the same port with the same data.
    pub fn restart(&mut self) {
        self.stop();
        self.spawn();
    }
}

/// Parse `"nats":["nats://127.0.0.1:PORT"]` out of the `*.ports` file.
fn read_port(dir: &std::path::Path) -> Option<u16> {
    let file = std::fs::read_dir(dir).ok()?.flatten().find(|e| e.path().extension().is_some_and(|x| x == "ports"))?;
    let text = std::fs::read_to_string(file.path()).ok()?;
    let at = text.find("nats://")?;
    let rest = &text[at + "nats://".len()..];
    let end = rest.find('"')?;
    rest[..end].rsplit_once(':')?.1.parse().ok()
}

impl Drop for NatsServer {
    fn drop(&mut self) {
        self.stop();
    }
}
