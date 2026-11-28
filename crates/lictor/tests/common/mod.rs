//! Helpers for end-to-end tests that run the real `lictor` binary.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

pub fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lictor"))
}

/// A command with a clean environment (no LICTOR_* leaking in from the developer's shell).
pub fn lictor(addr: &str) -> Command {
    let mut c = Command::new(bin());
    c.env_clear();
    for var in ["PATH", "HOME", "SYSTEMROOT", "TEMP", "TMP", "USERPROFILE", "LOCALAPPDATA"] {
        if let Some(v) = std::env::var_os(var) {
            c.env(var, v);
        }
    }
    c.env("LICTOR_HTTP_ADDR", addr).env("NO_COLOR", "1");
    c
}

/// An address where nothing listens (port 1 is privileged and unused on CI runners and dev machines).
pub const NOTHING_LISTENS: &str = "127.0.0.1:1";

pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {}\nstderr: {}", self.stdout, self.stderr))
    }

    pub fn ok(self) -> Run {
        assert_eq!(self.code, 0, "command failed\nstdout: {}\nstderr: {}", self.stdout, self.stderr);
        self
    }
}

impl From<Output> for Run {
    fn from(o: Output) -> Self {
        Run {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into(),
            stderr: String::from_utf8_lossy(&o.stderr).into(),
        }
    }
}

/// Run `lictor <args>` against the agent at `addr`.
pub fn run(addr: &str, args: &[&str]) -> Run {
    lictor(addr).args(args).output().expect("run lictor").into()
}

/// Run with stdin.
pub fn run_stdin(addr: &str, args: &[&str], input: &str) -> Run {
    use std::io::Write;
    let mut child =
        lictor(addr).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    child.wait_with_output().unwrap().into()
}

/// A running `lictor agent` process, killed on drop.
pub struct AgentProc {
    pub child: Child,
    pub addr: String,
    pub data_dir: tempfile::TempDir,
    pub log: PathBuf,
}

impl AgentProc {
    /// Start `lictor agent <args>` with its own port and data dir, and wait until it answers.
    pub fn start(args: &[&str]) -> AgentProc {
        Self::start_in(tempfile::tempdir().unwrap(), args)
    }

    pub fn start_in(data_dir: tempfile::TempDir, args: &[&str]) -> AgentProc {
        // Port 0: the agent picks a free port and logs it. Choosing one here and passing it in
        // races with other tests, which may take the port before the agent binds it.
        let n = std::fs::read_dir(data_dir.path()).map(|d| d.count()).unwrap_or(0);
        let log = data_dir.path().join(format!("agent-{n}.log"));
        let child = lictor("127.0.0.1:0")
            .arg("agent")
            .arg("--data-dir")
            .arg(data_dir.path())
            .args(args)
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .expect("spawn agent");
        let mut a = AgentProc { child, addr: String::new(), data_dir, log };
        a.addr = a.wait_ready();
        a
    }

    /// Wait for the agent's "HTTP API listening" log line and return the address it bound.
    /// The HTTP API starts after the agent has (catalog loaded, membership claimed).
    fn wait_ready(&mut self) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(addr) = listening_addr(&self.log_text()) {
                return addr;
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!("agent exited early ({status}):\n{}", self.log_text());
            }
            assert!(Instant::now() < deadline, "agent did not become ready:\n{}", self.log_text());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    pub fn run(&self, args: &[&str]) -> Run {
        run(&self.addr, args)
    }

    /// Send SIGTERM (graceful) and wait for the exit code.
    #[cfg(unix)]
    pub fn terminate(&mut self) -> i32 {
        Command::new("kill").arg("-TERM").arg(self.child.id().to_string()).status().unwrap();
        self.wait()
    }

    pub fn wait(&mut self) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                return s.code().unwrap_or(-1);
            }
            assert!(Instant::now() < deadline, "agent did not exit:\n{}", self.log_text());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn data_path(&self) -> &Path {
        self.data_dir.path()
    }
}

impl Drop for AgentProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Retry until `f` holds or 15 seconds pass.
pub fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The address in an "HTTP API listening addr=..." log line (text or JSON log format).
fn listening_addr(log: &str) -> Option<String> {
    let line = log.lines().find(|l| l.contains("HTTP API listening"))?;
    let rest = line.split_once("addr=").or_else(|| line.split_once("\"addr\":\""))?.1;
    let addr: String = rest.chars().take_while(|c| !c.is_whitespace() && *c != '"').collect();
    (!addr.is_empty()).then_some(addr)
}
