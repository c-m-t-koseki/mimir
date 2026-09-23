//! The engine server (`mimir-imserver`) as a child process, spoken to in
//! newline-delimited JSON-RPC 2.0 over its stdin/stdout — the protocol in
//! `mimir-im/core/src/server/protocol.rs`.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

/// How long a key may wait for the engine before it passes through
/// untouched: a stalled engine must not freeze typing in the application.
const KEY_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

/// `process_key` / `commit` result.
#[derive(Debug, Deserialize)]
pub struct KeyResult {
    pub consumed: bool,
    pub actions: Vec<Action>,
    /// "kana" or "ascii".
    #[serde(default)]
    pub mode: String,
    pub hint: Option<String>,
}

/// UI update from the engine.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    UpdatePreedit {
        text: String,
        /// Caret, in Unicode scalar values.
        caret: usize,
        #[serde(default)]
        attributes: Vec<PreeditAttr>,
    },
    ShowCandidates {
        candidates: Vec<Candidate>,
        cursor: usize,
        page: usize,
        total_pages: usize,
    },
    HideCandidates,
    Commit {
        text: String,
    },
    UpdateAux {
        text: String,
    },
    HideAux,
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct PreeditAttr {
    pub start: usize,
    pub end: usize,
    pub style: String,
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct Candidate {
    pub text: String,
    pub description: Option<String>,
}

pub struct EngineClient {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next_id: u64,
}

impl EngineClient {
    /// Start the server and ask it to load its models (in the background:
    /// keys convert kana-only until they are in).
    pub fn spawn(server: &Path) -> std::io::Result<Self> {
        let mut command = Command::new(server);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        Self::start(command)
    }

    fn start(mut command: Command) -> std::io::Result<Self> {
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            stdin: Some(stdin),
            lines,
            next_id: 0,
        };
        client.call("init", json!({}), Duration::from_secs(5));
        Ok(client)
    }

    /// Send a request and wait for its result; `None` on error, timeout,
    /// or a dead server.
    pub fn call(&mut self, method: &str, params: Value, timeout: Duration) -> Option<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let stdin = self.stdin.as_mut()?;
        writeln!(stdin, "{request}").ok()?;
        stdin.flush().ok()?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.checked_duration_since(Instant::now())?;
            let line = match self.lines.recv_timeout(left) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return None,
            };
            let mut response: Value = serde_json::from_str(&line).ok()?;
            // A late answer to a request that timed out: skip it.
            if response["id"] != json!(id) {
                continue;
            }
            return response.get_mut("result").map(Value::take);
        }
    }

    pub fn process_key(&mut self, keysym: u32, modifiers: Modifiers) -> Option<KeyResult> {
        let params = json!({
            "keysym": keysym,
            "modifiers": {"shift": modifiers.shift, "control": modifiers.control, "alt": modifiers.alt},
        });
        let result = self.call("process_key", params, KEY_TIMEOUT)?;
        serde_json::from_value(result).ok()
    }

    /// Commit whatever is open (focus leaving the text).
    pub fn commit(&mut self) -> Option<KeyResult> {
        let result = self.call("commit", json!({}), KEY_TIMEOUT)?;
        serde_json::from_value(result).ok()
    }

    /// Text left of the cursor, as conversion context.
    pub fn set_surrounding_text(&mut self, text: &str) {
        let params = json!({"text": text, "cursor_pos": text.chars().count()});
        self.call("set_surrounding_text", params, KEY_TIMEOUT);
    }
}

impl Drop for EngineClient {
    /// Closing stdin makes the server save its learning cache and exit.
    fn drop(&mut self) {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The server built next to this crate's tests (`cargo build -p
    /// mimir-im --bin mimir-imserver`); the test is skipped without it.
    fn server() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let dir = exe.parent()?.parent()?;
        let path = dir.join(format!("mimir-imserver{}", std::env::consts::EXE_SUFFIX));
        path.exists().then_some(path)
    }

    #[test]
    fn talks_to_the_server() {
        let Some(server) = server() else {
            eprintln!("mimir-imserver not built; skipped");
            return;
        };
        // A throwaway home: no config, no models, no learning written.
        let home = std::env::temp_dir().join(format!("mimir-tsf-test-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let mut command = Command::new(server);
        command
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("HF_HUB_OFFLINE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut client = EngineClient::start(command).unwrap();

        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let result = client.process_key(u32::from(b'K'), shift).unwrap();
        assert!(result.consumed);
        client
            .process_key(u32::from(b'a'), Modifiers::default())
            .unwrap();
        let result = client.process_key(0x20, Modifiers::default()).unwrap();
        assert!(
            result
                .actions
                .contains(&Action::Commit { text: "か".into() }),
            "{result:?}"
        );
        assert_eq!(result.mode, "kana");

        drop(client);
        let _ = std::fs::remove_dir_all(&home);
    }
}
