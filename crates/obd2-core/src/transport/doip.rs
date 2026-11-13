use crate::error::{CoreError, Result};
use crate::transport::DiagnosticTransport;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

pub const ROUTING_ACTIVATION_REQUEST: u16 = 0x0005;
pub const ROUTING_ACTIVATION_RESPONSE: u16 = 0x0006;
pub const DIAGNOSTIC_MESSAGE: u16 = 0x8001;
pub const DIAGNOSTIC_ACK: u16 = 0x8002;
pub const DIAGNOSTIC_NACK: u16 = 0x8003;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoipFrame {
    pub version: u8,
    pub payload_type: u16,
    pub payload: Vec<u8>,
}

impl DoipFrame {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + self.payload.len());
        out.push(self.version);
        out.push(!self.version);
        out.extend_from_slice(&self.payload_type.to_be_bytes());
        out.extend_from_slice(&(self.payload.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 8 {
            return Err(CoreError::Transport(
                "DoIP frame shorter than header".into(),
            ));
        }
        let version = bytes[0];
        if bytes[1] != !version {
            return Err(CoreError::Transport("DoIP version/inverse mismatch".into()));
        }
        let payload_type = u16::from_be_bytes([bytes[2], bytes[3]]);
        let length = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
        if length > 1_048_576 {
            return Err(CoreError::Transport(format!(
                "DoIP payload {length} is too large"
            )));
        }
        if bytes.len() < 8 + length {
            return Err(CoreError::Transport("DoIP frame truncated".into()));
        }
        Ok(Self {
            version,
            payload_type,
            payload: bytes[8..8 + length].to_vec(),
        })
    }
}

pub struct DoipClient {
    stream: TcpStream,
    source: u16,
    target: u16,
}

impl DoipClient {
    pub fn connect(address: &str, source: u16, target: u16, timeout: Duration) -> Result<Self> {
        let addr: SocketAddr = address
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| CoreError::Transport(format!("cannot resolve {address}")))?;
        let stream = TcpStream::connect_timeout(&addr, timeout)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        stream.set_nodelay(true)?;
        Ok(Self {
            stream,
            source,
            target,
        })
    }

    pub fn activate(&mut self, timeout: Duration) -> Result<()> {
        let mut payload = Vec::with_capacity(7);
        payload.extend_from_slice(&self.source.to_be_bytes());
        payload.push(0x00);
        payload.extend_from_slice(&[0u8; 4]);
        self.send(DoipFrame {
            version: 0x02,
            payload_type: ROUTING_ACTIVATION_REQUEST,
            payload,
        })?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(CoreError::Transport("routing activation timed out".into()));
            }
            let response = self.read_frame(remaining)?;
            match response.payload_type {
                0x0007 => {
                    self.send(DoipFrame {
                        version: 0x02,
                        payload_type: 0x0008,
                        payload: self.source.to_be_bytes().to_vec(),
                    })?;
                    continue;
                }
                ROUTING_ACTIVATION_RESPONSE => {
                    if response.payload.len() < 5 {
                        return Err(CoreError::Transport(
                            "short routing activation response".into(),
                        ));
                    }
                    let code = response.payload[4];
                    if code != 0x10 {
                        return Err(CoreError::Transport(format!(
                            "routing activation refused with code 0x{code:02x}"
                        )));
                    }
                    return Ok(());
                }
                _ => continue,
            }
        }
    }

    fn send(&mut self, frame: DoipFrame) -> Result<()> {
        self.stream.write_all(&frame.encode())?;
        self.stream.flush()?;
        Ok(())
    }

    fn read_frame(&mut self, timeout: Duration) -> Result<DoipFrame> {
        self.stream.set_read_timeout(Some(timeout))?;
        let mut header = [0u8; 8];
        self.stream.read_exact(&mut header)?;
        let length = u32::from_be_bytes(header[4..8].try_into().unwrap()) as usize;
        if length > 1_048_576 {
            return Err(CoreError::Transport(format!(
                "DoIP payload {length} is too large"
            )));
        }
        let mut body = vec![0u8; length];
        self.stream.read_exact(&mut body)?;
        let mut full = header.to_vec();
        full.extend_from_slice(&body);
        DoipFrame::decode(&full)
    }

    fn read_diagnostic(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(CoreError::Transport(
                    "DoIP diagnostic deadline elapsed".into(),
                ));
            }
            let frame = self.read_frame(remaining)?;
            match frame.payload_type {
                DIAGNOSTIC_ACK => continue,
                // Alive check request: reply with our tester address (0x0008).
                0x0007 => {
                    let mut payload = Vec::with_capacity(2);
                    payload.extend_from_slice(&self.source.to_be_bytes());
                    self.send(DoipFrame {
                        version: 0x02,
                        payload_type: 0x0008,
                        payload,
                    })?;
                    continue;
                }
                DIAGNOSTIC_NACK => {
                    let code = frame.payload.get(4).copied().unwrap_or(0xff);
                    return Err(CoreError::Transport(format!(
                        "DoIP negative ack 0x{code:02x}"
                    )));
                }
                DIAGNOSTIC_MESSAGE => {
                    if frame.payload.len() < 4 {
                        return Err(CoreError::Transport(
                            "DoIP diagnostic payload too short".into(),
                        ));
                    }
                    let source = u16::from_be_bytes([frame.payload[0], frame.payload[1]]);
                    let target = u16::from_be_bytes([frame.payload[2], frame.payload[3]]);
                    if source != self.target || target != self.source {
                        // A message for another tester/ECU during this session.
                        continue;
                    }
                    return Ok(frame.payload[4..].to_vec());
                }
                other => {
                    return Err(CoreError::Transport(format!(
                        "unexpected DoIP frame 0x{other:04x}"
                    )));
                }
            }
        }
    }
}

impl DiagnosticTransport for DoipClient {
    fn target_address(&self) -> Option<u16> {
        Some(self.target)
    }

    fn request(&mut self, frame: &[u8], timeout: Duration) -> Result<Vec<u8>> {
        let mut payload = Vec::with_capacity(4 + frame.len());
        payload.extend_from_slice(&self.source.to_be_bytes());
        payload.extend_from_slice(&self.target.to_be_bytes());
        payload.extend_from_slice(frame);
        self.send(DoipFrame {
            version: 0x02,
            payload_type: DIAGNOSTIC_MESSAGE,
            payload,
        })?;
        self.read_diagnostic(timeout)
    }

    fn receive(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        self.read_diagnostic(timeout)
    }
}
