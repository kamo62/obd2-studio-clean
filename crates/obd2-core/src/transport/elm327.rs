use crate::error::{CoreError, Result};
use crate::transport::DiagnosticTransport;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// ELM327-style serial adapter.
///
/// The adapter is modelled as any `Read + Write` stream so the same code works
/// with a USB serial device, a Bluetooth serial bridge or a TCP gateway. The
/// crate does not open serial ports itself; applications pass a stream in.
pub struct Elm327<S> {
    stream: S,
    buffer: Vec<u8>,
    initialized: bool,
}

impl<S: Read + Write> Elm327<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            buffer: Vec::new(),
            initialized: false,
        }
    }

    /// Send the adapter reset and a neutral configuration.
    ///
    /// Echo, line feeds, spaces and headers are disabled so responses are
    /// compact and deterministic. Protocol selection is left automatic.
    pub fn initialize(&mut self, timeout: Duration) -> Result<()> {
        for command in ["ATZ", "ATE0", "ATL0", "ATS0", "ATH0", "ATSP0"] {
            self.send_command(command, timeout)?;
        }
        self.initialized = true;
        Ok(())
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    fn send_command(&mut self, command: &str, timeout: Duration) -> Result<String> {
        self.stream.write_all(command.as_bytes())?;
        self.stream.write_all(b"\r")?;
        self.stream.flush()?;
        self.read_until_prompt(timeout)
    }

    fn read_until_prompt(&mut self, timeout: Duration) -> Result<String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(position) = self.buffer.iter().position(|byte| *byte == b'>') {
                let line = self.buffer.drain(..=position).collect::<Vec<_>>();
                return Ok(normalize(&line));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(CoreError::Transport("ELM327 timed out".into()));
            }
            let mut chunk = [0u8; 256];
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    return Err(CoreError::Transport("ELM327 closed the connection".into()));
                }
                Ok(read) => self.buffer.extend_from_slice(&chunk[..read]),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Err(CoreError::Transport("ELM327 read timed out".into()));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn exchange(&mut self, frame: &[u8], timeout: Duration) -> Result<Vec<u8>> {
        if frame.is_empty() {
            return Err(CoreError::Invalid("empty diagnostic request".into()));
        }
        let command = frame
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
        let response = self.send_command(&command, timeout)?;
        parse_response(&response)
    }
}

/// A TCP gateway around an ELM327-compatible adapter, for WiFi adapters and
/// serial-to-network bridges.
impl Elm327<std::net::TcpStream> {
    pub fn connect_tcp(address: &str, timeout: Duration) -> Result<Self> {
        use std::net::ToSocketAddrs;
        let addr = address
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| CoreError::Transport(format!("cannot resolve {address}")))?;
        let stream = std::net::TcpStream::connect_timeout(&addr, timeout)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        stream.set_nodelay(true)?;
        Ok(Self::new(stream))
    }
}

impl<S: Read + Write> DiagnosticTransport for Elm327<S> {
    fn request(&mut self, frame: &[u8], timeout: Duration) -> Result<Vec<u8>> {
        if !self.initialized {
            self.initialize(timeout)?;
        }
        self.exchange(frame, timeout)
    }

    fn receive(&mut self, _timeout: Duration) -> Result<Vec<u8>> {
        Err(CoreError::Transport(
            "ELM327 adapters do not expose response-pending frames".into(),
        ))
    }
}

/// Parse the text returned by the adapter into the diagnostic payload.
///
/// Handles an echoed command, status lines (`SEARCHING...`), CAN headers and
/// adapter error messages. Multi-line payloads are concatenated.
pub fn parse_response(text: &str) -> Result<Vec<u8>> {
    let mut payload = String::new();
    for line in text.split(|ch| ch == '\r' || ch == '\n') {
        let line = line.trim();
        if line.is_empty() || line == ">" {
            continue;
        }
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("SEARCHING") || upper.starts_with("BUS INIT") {
            continue;
        }
        if upper.contains("NO DATA") || upper == "?" || upper.starts_with("STOPPED") {
            return Err(CoreError::Transport(format!("adapter response: {line}")));
        }
        let compact: String = line.chars().filter(|ch| ch.is_ascii_hexdigit()).collect();
        if compact.is_empty() {
            // A status line with no hex digits, such as "OK" or "ELM327 v1.5".
            continue;
        }
        if compact.len() % 2 != 0 {
            return Err(CoreError::Transport(format!(
                "adapter returned a nibble-aligned line: {line}"
            )));
        }
        if compact.len() == 2 && payload.is_empty() {
            // A bare status byte such as "7F" is not a complete response.
            continue;
        }
        payload.push_str(&compact);
    }
    if payload.is_empty() {
        return Err(CoreError::Transport("adapter returned no data".into()));
    }
    crate::util::decode_hex_strict(&payload, 65_535)
}

fn normalize(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.replace('\r', "\n")
}

/// Split a CAN response that includes a PCI byte or headers.
///
/// ELM327 normally removes ISO-TP framing, but some adapters leave a length
/// byte or a three-byte header in place. This helper strips a single-byte PCI
/// and an 11-bit CAN header when the payload length matches.
pub fn strip_can_framing(data: &[u8]) -> Result<Vec<u8>> {
    if data.is_empty() {
        return Err(CoreError::Transport("empty CAN response".into()));
    }
    let pci = data[0];
    if pci <= 0x07 {
        let expected = pci as usize + 1;
        if data.len() == expected {
            return Ok(data[1..].to_vec());
        }
    }
    if pci & 0xF0 == 0x10 && data.len() >= 3 {
        let length = (((pci & 0x0F) as usize) << 8) | data[1] as usize;
        if data.len() >= length + 2 {
            return Ok(data[2..2 + length].to_vec());
        }
    }
    Ok(data.to_vec())
}
