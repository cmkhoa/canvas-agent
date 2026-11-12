//! Append-only JSONL audit log.
//!
//! Records every request (hashed), tool call, refusal, deterministic priority calculation,
//! validation, and final response (hashed). Secrets and full Canvas content are not written.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::error::{AgentError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub ts: DateTime<Utc>,
    pub request_id: String,
    pub turn: u32,
    pub event: String,
    pub data: Value,
}

pub struct AuditLog {
    path: PathBuf,
    file: Mutex<File>,
}

impl AuditLog {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| {
                AgentError::Audit(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| AgentError::Audit(format!("cannot open {}: {e}", path.display())))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
        Ok(Self {
            path: path.to_path_buf(),
            file: Mutex::new(file),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, request_id: &str, turn: u32, event: &str, data: Value) -> Result<()> {
        let record = AuditEvent {
            ts: Utc::now(),
            request_id: request_id.to_string(),
            turn,
            event: event.to_string(),
            data,
        };
        let mut line = serde_json::to_string(&record)
            .map_err(|e| AgentError::Audit(format!("cannot serialize audit event: {e}")))?;
        line.push('\n');
        let mut file = self
            .file
            .lock()
            .map_err(|_| AgentError::Audit("audit log lock poisoned".into()))?;
        file.write_all(line.as_bytes())
            .and_then(|_| file.flush())
            .map_err(|e| AgentError::Audit(format!("cannot append to audit log: {e}")))
    }

    /// Test/eval helper. Production code only appends.
    pub fn read_events(&self) -> Result<Vec<AuditEvent>> {
        let file = File::open(&self.path)
            .map_err(|e| AgentError::Audit(format!("cannot read audit log: {e}")))?;
        let mut events = Vec::new();
        for line in BufReader::new(file).lines() {
            let line =
                line.map_err(|e| AgentError::Audit(format!("cannot read audit line: {e}")))?;
            if line.trim().is_empty() {
                continue;
            }
            let event = serde_json::from_str(&line)
                .map_err(|e| AgentError::Audit(format!("invalid audit line: {e}")))?;
            events.push(event);
        }
        Ok(events)
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub fn input_fingerprint(input: &str) -> Value {
    json!({
        "sha256": sha256_hex(input.as_bytes()),
        "chars": input.chars().count(),
    })
}

/// A process-local request ID without adding a UUID dependency.
pub fn request_id(now: DateTime<Utc>, turn: u32) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("req-{}-{turn}-{n}", now.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_valid_jsonl_and_redacts_nothing_into_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let log = AuditLog::open(&path).unwrap();
        log.append("req-1", 1, "request", input_fingerprint("secret question"))
            .unwrap();
        log.append("req-1", 1, "final_response", json!({"sha256": "abc"}))
            .unwrap();
        let events = log.read_events().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event, "request");
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("secret question"));
        assert!(raw.contains("\"sha256\""));
    }

    #[test]
    fn hashes_are_stable() {
        assert_eq!(
            sha256_hex(b"canvas"),
            "a457e8f04aa796dc50770f109d184aaa9b389b8429eda73d49bd7dfdb199aa9c"
        );
        assert_eq!(sha256_hex(b"").len(), 64);
        assert_ne!(sha256_hex(b"a"), sha256_hex(b"b"));
    }

    #[test]
    fn request_ids_are_unique_per_call() {
        let now = Utc::now();
        assert_ne!(request_id(now, 1), request_id(now, 1));
    }
}
