//! A live debug session: a `gopher2600-mcp` child driven over the framed protocol.
//!
//! Each [`Session`] owns a headless Gopher2600 debugger process. Commands are
//! written to the child's stdin one line at a time; the child's stdout is read
//! until the matching protocol frame closes (see [`crate::session`] markers).
//! A single [`tokio::sync::Mutex`] around the connection serialises commands so
//! concurrent tool calls cannot interleave on the same session.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::time::timeout;

/// ASCII Record Separator that fences every protocol marker emitted by `mcpterm`.
const RS: char = '\u{1e}';

/// Default time to wait for a single command's output frame to close.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// Outcome of a single debugger command.
#[derive(Debug, Clone)]
pub struct CommandResult {
    /// The command's output, with protocol markers stripped, newline-joined.
    pub text: String,
    /// Whether the debugger reported any error-styled output for the command.
    pub error: bool,
}

/// Connection state guarded by a mutex so only one command runs at a time.
#[derive(Debug)]
struct Conn {
    stdin: ChildStdin,
    reader: Lines<BufReader<ChildStdout>>,
}

/// A running headless Gopher2600 debug session for one ROM.
#[derive(Debug)]
pub struct Session {
    /// Stable identifier assigned by the server (e.g. `s1`).
    pub id: String,
    /// The ROM this session was started with.
    pub rom: PathBuf,
    /// Operating-system process id of the child, used for signalling.
    pub pid: u32,
    /// Output emitted while the ROM was loading, before the first command.
    pub startup: String,
    child: Mutex<Child>,
    conn: Mutex<Conn>,
}

impl Session {
    /// Spawns a debug session for `rom` using the helper binary at `bin`.
    ///
    /// # Errors
    ///
    /// Returns an error if the child cannot be spawned, its pipes cannot be
    /// captured, or the startup frame does not arrive before the timeout.
    pub async fn start(
        id: impl Into<String>,
        bin: &Path,
        rom: &Path,
        mapping: Option<&str>,
        tv: Option<&str>,
    ) -> Result<Self> {
        let mut cmd = Command::new(bin);
        if let Some(m) = mapping {
            cmd.arg("-mapping").arg(m);
        }
        if let Some(t) = tv {
            cmd.arg("-tv").arg(t);
        }
        cmd.arg(rom);

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to launch helper: {}", bin.display()))?;

        let pid = child.id().ok_or_else(|| anyhow!("child has no pid"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("no child stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("no child stdout"))?;
        let mut conn = Conn {
            stdin,
            reader: BufReader::new(stdout).lines(),
        };

        // The ROM-loading output arrives as the seq=0 frame; consume it so the
        // session is positioned to accept commands. A loader failure surfaces
        // here as error-styled output.
        let startup = collect_frame(&mut conn, DEFAULT_TIMEOUT)
            .await
            .context("waiting for session startup")?;
        if startup.error {
            // keep the session so the caller can read the diagnostic, but report it.
            return Err(anyhow!("emulator failed to start:\n{}", startup.text));
        }

        Ok(Self {
            id: id.into(),
            rom: rom.to_path_buf(),
            pid,
            startup: startup.text,
            child: Mutex::new(child),
            conn: Mutex::new(conn),
        })
    }

    /// Sends one debugger command line and returns its output.
    ///
    /// # Errors
    ///
    /// Returns an error if the child has exited or the response frame does not
    /// close within the timeout (e.g. an unbounded `RUN` with no breakpoint).
    pub async fn send(&self, command: &str) -> Result<CommandResult> {
        let mut conn = self.conn.lock().await;
        write_command(&mut conn.stdin, command).await?;
        collect_frame(&mut conn, DEFAULT_TIMEOUT).await
    }

    /// Resumes free-running emulation, auto-halting after `run` seconds.
    ///
    /// Emulation continues until a halt condition fires (breakpoint, trap,
    /// watch) or `run` elapses. On timeout an interrupt is sent, which the
    /// debugger turns into a clean halt back to the prompt (see the helper's
    /// terminal). If a halt occurs first, no interrupt is sent.
    ///
    /// # Errors
    ///
    /// Returns an error if the child has exited or no halt frame arrives within
    /// the grace period after the interrupt.
    pub async fn run(&self, run: Duration) -> Result<CommandResult> {
        let mut conn = self.conn.lock().await;
        write_command(&mut conn.stdin, "RUN").await?;

        // Wait for a natural halt; only interrupt if none arrives in time. While
        // running, the emulation produces no output, so abandoning the first
        // collect on timeout loses nothing.
        tokio::select! {
            res = collect_frame(&mut conn, run + Duration::from_secs(10)) => res,
            () = tokio::time::sleep(run) => {
                send_interrupt(self.pid).await?;
                collect_frame(&mut conn, Duration::from_secs(10)).await
            }
        }
    }

    /// Sends SIGINT to break a running emulation back into the debugger.
    ///
    /// # Errors
    ///
    /// Returns an error if the signal cannot be delivered.
    pub async fn interrupt(&self) -> Result<()> {
        send_interrupt(self.pid).await
    }

    /// Quits the debugger and ensures the child process is gone.
    pub async fn stop(&self) {
        {
            let mut conn = self.conn.lock().await;
            let _ = write_command(&mut conn.stdin, "QUIT").await;
        }
        let mut child = self.child.lock().await;
        let _ = timeout(Duration::from_secs(2), child.wait()).await;
        let _ = child.start_kill();
    }
}

/// Writes a command line (terminated with a newline) and flushes it.
async fn write_command(stdin: &mut ChildStdin, command: &str) -> Result<()> {
    let line = format!("{}\n", command.trim_end());
    stdin
        .write_all(line.as_bytes())
        .await
        .context("writing command to emulator")?;
    stdin.flush().await.context("flushing command")?;
    Ok(())
}

/// Reads lines until the current command's `END` marker, stripping protocol framing.
async fn collect_frame(conn: &mut Conn, limit: Duration) -> Result<CommandResult> {
    let end_marker = format!("{RS}END");
    let begin_marker = format!("{RS}BEGIN");
    let err_marker = format!("{RS}ERR{RS} ");
    let prompt_marker = format!("{RS}PROMPT{RS} ");

    let mut lines: Vec<String> = Vec::new();
    let mut position: Option<String> = None;
    let mut error = false;

    loop {
        let next = timeout(limit, conn.reader.next_line())
            .await
            .map_err(|_| anyhow!("timed out after {limit:?} waiting for command output"))?
            .context("reading emulator output")?;

        let Some(line) = next else {
            bail!("emulator process closed its output stream");
        };

        if let Some(rest) = line.strip_prefix(&end_marker) {
            // marker form: "<RS>END seq=<n> err=<0|1><RS>"
            if rest.contains("err=1") {
                error = true;
            }
            break;
        }
        if line.starts_with(&begin_marker) {
            continue;
        }
        if let Some(rest) = line.strip_prefix(&err_marker) {
            error = true;
            lines.push(rest.to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix(&prompt_marker) {
            position = Some(rest.to_string());
            continue;
        }
        lines.push(line);
    }

    // surface the post-command position (next instruction) as a trailing line,
    // unless the command's own output already ends with it.
    if let Some(pos) = position
        && lines.last().map(String::as_str) != Some(pos.as_str())
    {
        lines.push(format!("\u{2192} {pos}"));
    }

    Ok(CommandResult {
        text: lines.join("\n"),
        error,
    })
}

/// Sends SIGINT to a process id using the system `kill` utility.
///
/// Shelling out avoids an `unsafe` libc FFI call for a one-shot signal.
async fn send_interrupt(pid: u32) -> Result<()> {
    let status = Command::new("kill")
        .arg("-INT")
        .arg(pid.to_string())
        .status()
        .await
        .context("invoking kill")?;
    if !status.success() {
        bail!("kill -INT {pid} failed");
    }
    Ok(())
}
