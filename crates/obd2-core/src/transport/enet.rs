use crate::error::{CoreError, Result};
use crate::transport::DiagnosticTransport;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// BMW ENET/HSFZ framing:
/// `u32 length | u16 control | u8 source | u8 target | payload`
/// where length covers `source + target + payload`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnetFrame {
    pub control: u16,
    pub source: u8,
    pub target: u8,
    pub payload: Vec<u8>,
}

impl EnetFrame {
    pub fn encode(&self) -> Vec<u8> {
        let length = 2 + self.payload.len();
        let mut out = Vec::with_capacity(8 + self.payload.len());
        out.extend_from_slice(&(length as u32).to_be_bytes());
        out.extend_from_slice(&self.control.to_be_bytes());
        out.push(self.source);
        out.push(self.target);
        out.extend_from_slice(&self.payload);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 8 {
            return Err(CoreError::Transport(
                "ENET frame shorter than header".into(),
            ));
        }
        let length = u32::from_be_bytes(bytes[0..4].try_into().unwrap()) as usize;
        if !(2..=65_537).contains(&length) {
            return Err(CoreError::Transport(format!(
                "ENET frame length {length} is invalid"
            )));
        }
        if bytes.len() < 8 + (length - 2) {
            return Err(CoreError::Transport("ENET frame truncated".into()));
        }
        let control = u16::from_be_bytes(bytes[4..6].try_into().unwrap());
        let source = bytes[6];
        let target = bytes[7];
        Ok(Self {
            control,
            source,
            target,
            payload: bytes[8..8 + length - 2].to_vec(),
        })
    }
}

pub struct EnetClient {
    stream: TcpStream,
    source: u8,
    target: u8,
}

impl EnetClient {
    pub fn connect(address: &str, source: u8, target: u8, timeout: Duration) -> Result<Self> {
        let addr = resolve(address)?;
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

    fn read_diagnostic(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        for _ in 0..8 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let response = self.read_frame(remaining)?;
            // HSFZ control 0x0001 is a diagnostic frame; ACKs and alive checks
            // are not the UDS answer and must not be returned to the job layer.
            if response.control == 0x0001
                && response.source == self.target
                && response.target == self.source
            {
                return Ok(response.payload);
            }
        }
        Err(CoreError::Transport(
            "no diagnostic response from the target ECU".into(),
        ))
    }

    fn read_frame(&mut self, timeout: Duration) -> Result<EnetFrame> {
        self.stream.set_read_timeout(Some(timeout))?;
        let mut header = [0u8; 4];
        self.stream.read_exact(&mut header)?;
        let length = u32::from_be_bytes(header) as usize;
        if !(2..=65_537).contains(&length) {
            return Err(CoreError::Transport(format!(
                "ENET frame length {length} is invalid"
            )));
        }
        let mut rest = vec![0u8; 4 + length - 2];
        self.stream.read_exact(&mut rest)?;
        let mut full = header.to_vec();
        full.extend_from_slice(&rest);
        EnetFrame::decode(&full)
    }
}

impl DiagnosticTransport for EnetClient {
    fn request(&mut self, frame: &[u8], timeout: Duration) -> Result<Vec<u8>> {
        let bytes = EnetFrame {
            control: 0x0001,
            source: self.source,
            target: self.target,
            payload: frame.to_vec(),
        }
        .encode();
        self.stream.set_write_timeout(Some(timeout))?;
        self.stream.write_all(&bytes)?;
        self.stream.flush()?;
        self.read_diagnostic(timeout)
    }

    fn receive(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        self.read_diagnostic(timeout)
    }

    fn target_address(&self) -> Option<u16> {
        Some(self.target as u16)
    }
}

fn resolve(address: &str) -> Result<SocketAddr> {
    address
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| CoreError::Transport(format!("cannot resolve {address}")))
}
