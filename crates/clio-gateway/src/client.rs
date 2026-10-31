use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use crate::types::{ActionRequest, Decision};

/// The loop's side of the separate-process gateway (S-11): one JSON request per line out, one
/// decision per line back. Any failure to get a decision is a deny, so with the process down
/// nothing runs.
pub struct GatewayClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl GatewayClient {
    pub fn spawn(binary: &str, rules_path: &str) -> std::io::Result<Self> {
        let mut child = Command::new(binary)
            .arg(rules_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let missing = || std::io::Error::other("no pipe");
        let stdin = child.stdin.take().ok_or_else(missing)?;
        let stdout = BufReader::new(child.stdout.take().ok_or_else(missing)?);
        Ok(Self {
            child,
            stdin,
            stdout,
        })
    }

    pub fn decide(&mut self, req: &ActionRequest) -> Decision {
        self.try_decide(req)
            .unwrap_or_else(|e| Decision::fail_closed(format!("gateway process unavailable: {e}")))
    }

    fn try_decide(&mut self, req: &ActionRequest) -> Result<Decision, String> {
        let line = serde_json::to_string(req).map_err(|e| e.to_string())?;
        writeln!(self.stdin, "{line}")
            .and_then(|()| self.stdin.flush())
            .map_err(|e| e.to_string())?;
        let mut out = String::new();
        match self.stdout.read_line(&mut out) {
            Ok(0) => Err("process closed its output".into()),
            Ok(_) => serde_json::from_str(&out).map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// For tests: take the process down.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
