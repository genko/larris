use std::{
    io::{BufRead, BufReader, Write},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serialport::{DataBits, FlowControl, Parity, SerialPort, StopBits};
use tokio::sync::mpsc;

use crate::app::BaudRate;

// ── Port discovery ────────────────────────────────────────────────────────────

/// Prefixes we consider "interesting" serial devices on Linux.
const SERIAL_PREFIXES: &[&str] = &["ttyUSB", "ttyACM", "ttyS", "ttyAMA", "rfcomm"];

/// Return a sorted list of likely serial port paths found under /dev.
pub fn discover_ports() -> Vec<String> {
    let Ok(read_dir) = std::fs::read_dir("/dev") else {
        return Vec::new();
    };

    let mut ports: Vec<String> = read_dir
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            let is_serial = SERIAL_PREFIXES
                .iter()
                .any(|prefix| name_str.starts_with(prefix));
            if is_serial {
                Some(format!("/dev/{name_str}"))
            } else {
                None
            }
        })
        .collect();

    ports.sort();
    ports
}

// ── Connection ────────────────────────────────────────────────────────────────

/// Settings that are fixed for GRBL connections:
/// 8 data bits, 1 stop bit, no parity, no flow control.
fn open_port(path: &str, baud: BaudRate) -> Result<Box<dyn SerialPort>> {
    serialport::new(path, baud.as_u32())
        .data_bits(DataBits::Eight)
        .stop_bits(StopBits::One)
        .parity(Parity::None)
        .flow_control(FlowControl::None)
        .timeout(Duration::from_millis(10))
        .open()
        .with_context(|| format!("Failed to open serial port {path} at {baud}"))
}

// ── Actor messages ────────────────────────────────────────────────────────────

/// Messages the serial actor sends back to the UI.
#[derive(Debug)]
pub enum SerialEvent {
    /// A complete line (without the trailing newline) received from the device.
    Line(String),
    /// The connection was closed (either by the user or due to an error).
    Disconnected(Option<String>),
    /// An informational / error message to surface in the console.
    Info(String),
    /// An error message to surface in the console.
    Error(String),
    /// A GCode stream is in progress: `sent` lines acknowledged so far out of `total`.
    StreamProgress { sent: usize, total: usize },
    /// The GCode stream finished successfully.
    StreamDone { total: usize },
    /// The GCode stream was aborted (by the user or due to an error).
    StreamAborted {
        sent: usize,
        total: usize,
        reason: String,
    },
}

/// Commands the UI sends to the serial actor.
#[derive(Debug)]
pub enum SerialCommand {
    /// Send a raw string to the device (a `\n` is appended automatically).
    Send(String),
    /// Stream a list of GCode lines using GRBL's ok-gated simple-sender
    /// protocol: send one line, wait for `ok` or `error:N`, then send the
    /// next.  Progress events are emitted for every acknowledgement.
    Stream(Vec<String>),
    /// Abort an in-progress Stream (the current line finishes but no further
    /// lines are sent).
    AbortStream,
    /// Close the port and shut down the actor.
    Disconnect,
}

// ── Serial actor ──────────────────────────────────────────────────────────────

/// Spawn a blocking thread that owns the serial port.
///
/// Returns:
/// - `tx` – send [`SerialCommand`]s to the actor.
/// - `rx` – receive [`SerialEvent`]s from the actor.
pub fn spawn_serial_actor(
    path: &str,
    baud: BaudRate,
) -> Result<(
    mpsc::UnboundedSender<SerialCommand>,
    mpsc::UnboundedReceiver<SerialEvent>,
)> {
    let port = open_port(path, baud)?;

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<SerialCommand>();
    let (evt_tx, evt_rx) = mpsc::unbounded_channel::<SerialEvent>();

    let path_owned = path.to_owned();

    // Run the blocking I/O on a dedicated OS thread so we don't block the
    // async runtime.
    std::thread::spawn(move || {
        serial_actor_loop(port, path_owned, cmd_rx, evt_tx);
    });

    Ok((cmd_tx, evt_rx))
}

/// Blocking loop that reads from the port and dispatches commands.
///
/// This runs entirely on a dedicated OS thread.
fn serial_actor_loop(
    port: Box<dyn SerialPort>,
    path: String,
    mut cmd_rx: mpsc::UnboundedReceiver<SerialCommand>,
    evt_tx: mpsc::UnboundedSender<SerialEvent>,
) {
    // We need two handles to the port: one for reading, one for writing.
    let write_port = match port.try_clone() {
        Ok(p) => p,
        Err(e) => {
            let _ = evt_tx.send(SerialEvent::Error(format!(
                "Failed to clone port for writing: {e}"
            )));
            let _ = evt_tx.send(SerialEvent::Disconnected(Some(
                "Could not clone serial port".into(),
            )));
            return;
        }
    };

    let _ = evt_tx.send(SerialEvent::Info(format!("Connected to {path}")));

    // Spawn a secondary thread for writes so reads are never blocked by I/O.
    let (write_tx, write_rx) = std::sync::mpsc::channel::<Option<String>>();
    let evt_tx_writer = evt_tx.clone();
    std::thread::spawn(move || {
        writer_loop(write_port, write_rx, evt_tx_writer);
    });

    // Read loop – runs on this thread.
    let mut reader = BufReader::new(port);
    let mut line_buf = String::new();

    // When Some(_), we are in streaming mode.
    let mut stream: Option<StreamState> = None;

    loop {
        // ── Check for pending commands (non-blocking) ─────────────────────
        loop {
            match cmd_rx.try_recv() {
                Ok(SerialCommand::Send(text)) => {
                    if stream.is_some() {
                        // Ignore raw sends during streaming to avoid
                        // confusing GRBL's ok sequencing.
                        let _ = evt_tx.send(SerialEvent::Info(
                            "Raw send ignored while streaming – abort stream first.".into(),
                        ));
                    } else if write_tx.send(Some(text)).is_err() {
                        break;
                    }
                }
                Ok(SerialCommand::Stream(lines)) => {
                    if stream.is_some() {
                        let _ = evt_tx.send(SerialEvent::Info(
                            "Already streaming – ignoring duplicate Stream command.".into(),
                        ));
                    } else if lines.is_empty() {
                        let _ = evt_tx.send(SerialEvent::StreamDone { total: 0 });
                    } else {
                        let total = lines.len();
                        let _ = evt_tx.send(SerialEvent::Info(format!(
                            "Starting GCode stream: {total} lines"
                        )));
                        // Send the very first line immediately.
                        let first = lines[0].clone();
                        if write_tx.send(Some(first)).is_err() {
                            break;
                        }
                        stream = Some(StreamState {
                            lines,
                            next_idx: 1, // next line to send after ack
                            sent: 0,     // lines acknowledged so far
                            abort: false,
                        });
                    }
                }
                Ok(SerialCommand::AbortStream) => {
                    if let Some(ref st) = stream {
                        let _ = evt_tx.send(SerialEvent::StreamAborted {
                            sent: st.sent,
                            total: st.lines.len(),
                            reason: "Aborted by user".into(),
                        });
                        stream = None;
                    }
                }
                Ok(SerialCommand::Disconnect) => {
                    let _ = write_tx.send(None);
                    let _ = evt_tx.send(SerialEvent::Disconnected(None));
                    return;
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    let _ = write_tx.send(None);
                    let _ = evt_tx.send(SerialEvent::Disconnected(None));
                    return;
                }
            }
        }

        // ── Try to read a line ────────────────────────────────────────────
        line_buf.clear();
        match reader.read_line(&mut line_buf) {
            Ok(0) => {
                // EOF – port closed by device
                let _ = write_tx.send(None);
                let _ = evt_tx.send(SerialEvent::Error("Serial port closed by device".into()));
                let _ = evt_tx.send(SerialEvent::Disconnected(Some(
                    "Port closed by device".into(),
                )));
                return;
            }
            Ok(_) => {
                let text = line_buf.trim_end_matches(['\r', '\n']).to_string();
                if text.is_empty() {
                    continue;
                }

                // Always forward the raw line to the UI.
                let _ = evt_tx.send(SerialEvent::Line(text.clone()));

                // ── ok-gated streaming ────────────────────────────────────
                if let Some(ref mut st) = stream {
                    let is_ok = text.eq_ignore_ascii_case("ok");
                    let is_error = text.to_ascii_lowercase().starts_with("error:");

                    if is_ok || is_error {
                        st.sent += 1;
                        let total = st.lines.len();

                        if is_error {
                            // On error: report and abort the stream.
                            let _ = evt_tx.send(SerialEvent::StreamAborted {
                                sent: st.sent,
                                total,
                                reason: format!(
                                    "GRBL reported '{}' on line {} of {}",
                                    text, st.sent, total
                                ),
                            });
                            stream = None;
                            continue;
                        }

                        // Emit progress.
                        let _ = evt_tx.send(SerialEvent::StreamProgress {
                            sent: st.sent,
                            total,
                        });

                        if st.abort || st.next_idx >= total {
                            // All lines sent and acknowledged – done.
                            let _ = evt_tx.send(SerialEvent::StreamDone { total: st.sent });
                            stream = None;
                        } else {
                            // Send the next line.
                            let next = st.lines[st.next_idx].clone();
                            st.next_idx += 1;
                            if write_tx.send(Some(next)).is_err() {
                                let _ = evt_tx.send(SerialEvent::StreamAborted {
                                    sent: st.sent,
                                    total,
                                    reason: "Writer thread died".into(),
                                });
                                stream = None;
                            }
                        }
                    }
                    // Non-ok/error lines (status reports, messages) are
                    // already forwarded above; streaming is not affected.
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::TimedOut
                    || e.kind() == std::io::ErrorKind::WouldBlock =>
            {
                // No data available yet – yield briefly and loop
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => {
                let _ = write_tx.send(None);
                let _ = evt_tx.send(SerialEvent::Error(format!("Read error: {e}")));
                let _ = evt_tx.send(SerialEvent::Disconnected(Some(format!("Read error: {e}"))));
                return;
            }
        }
    }
}

/// State kept while streaming GCode line by line.
struct StreamState {
    /// All lines to be sent.
    lines: Vec<String>,
    /// Index of the next line to send (after the current ack).
    next_idx: usize,
    /// Number of lines acknowledged so far.
    sent: usize,
    /// When true, stop after the current in-flight line is acked.
    abort: bool,
}

/// Blocking loop that writes lines to the serial port.
fn writer_loop(
    mut port: Box<dyn SerialPort>,
    rx: std::sync::mpsc::Receiver<Option<String>>,
    evt_tx: mpsc::UnboundedSender<SerialEvent>,
) {
    for msg in &rx {
        match msg {
            None => return, // Shutdown signal
            Some(text) => {
                let to_send = format!("{text}\n");
                if let Err(e) = port.write_all(to_send.as_bytes()) {
                    let _ = evt_tx.send(SerialEvent::Error(format!("Write error: {e}")));
                    return;
                }
                if let Err(e) = port.flush() {
                    let _ = evt_tx.send(SerialEvent::Error(format!("Flush error: {e}")));
                    return;
                }
            }
        }
    }
}

// ── Validate helpers (unit-testable) ─────────────────────────────────────────

/// Sanity-check a path before trying to open it.
pub fn validate_port_path(path: &str) -> Result<()> {
    if !path.starts_with("/dev/") {
        bail!("Port path must start with /dev/");
    }
    if !std::path::Path::new(path).exists() {
        bail!("Device {path} does not exist");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_returns_sorted_list() {
        // Just ensure it doesn't panic and returns a Vec (may be empty in CI)
        let ports = discover_ports();
        let mut sorted = ports.clone();
        sorted.sort();
        assert_eq!(ports, sorted);
    }

    #[test]
    fn validate_rejects_non_dev_paths() {
        assert!(validate_port_path("/tmp/fake").is_err());
        assert!(validate_port_path("ttyUSB0").is_err());
    }

    #[test]
    fn validate_rejects_missing_device() {
        assert!(validate_port_path("/dev/ttyUSB_does_not_exist_xyz").is_err());
    }
}
