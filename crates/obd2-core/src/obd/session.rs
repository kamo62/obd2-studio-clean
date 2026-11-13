use crate::error::{CoreError, Result};
use crate::obd::dtc::{self, Dtc};
use crate::transport::DiagnosticTransport;
use std::time::Duration;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DtcMode {
    Stored,
    Pending,
    Permanent,
}

impl DtcMode {
    pub fn service(self) -> u8 {
        match self {
            Self::Stored => 0x03,
            Self::Pending => 0x07,
            Self::Permanent => 0x0A,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Stored => "Stored",
            Self::Pending => "Pending",
            Self::Permanent => "Permanent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MilStatus {
    pub mil_on: bool,
    pub dtc_count: u8,
    pub readiness: [u8; 3],
}

pub struct ObdSession<'a> {
    transport: &'a mut dyn DiagnosticTransport,
    timeout: Duration,
}

impl<'a> ObdSession<'a> {
    pub fn new(transport: &'a mut dyn DiagnosticTransport, timeout: Duration) -> Self {
        Self { transport, timeout }
    }

    pub fn with_default_timeout(transport: &'a mut dyn DiagnosticTransport) -> Self {
        Self::new(transport, DEFAULT_TIMEOUT)
    }

    pub fn request(&mut self, request: &[u8]) -> Result<Vec<u8>> {
        if request.is_empty() {
            return Err(CoreError::Invalid("empty diagnostic request".into()));
        }
        let service = request[0];
        let mut response = self.transport.request(request, self.timeout)?;
        let mut pending = 0;
        while response.len() >= 3
            && response[0] == 0x7F
            && response[1] == service
            && response[2] == 0x78
        {
            pending += 1;
            if pending > 64 {
                return Err(CoreError::Transport(
                    "ECU kept requesting more time (0x78)".into(),
                ));
            }
            response = self.transport.receive(self.timeout)?;
        }
        if response.len() >= 3 && response[0] == 0x7F && response[1] == service {
            let code = response[2];
            return Err(CoreError::NegativeResponse {
                service,
                code,
                message: nrc_message(code).into(),
            });
        }
        if response.first().copied() != Some(service.wrapping_add(0x40)) {
            return Err(CoreError::Transport(format!(
                "expected response 0x{:02X}, got 0x{:02X}",
                service.wrapping_add(0x40),
                response.first().copied().unwrap_or(0)
            )));
        }
        Ok(response)
    }

    pub fn read_pid(&mut self, pid: u8) -> Result<Vec<u8>> {
        let response = self.request(&[0x01, pid])?;
        if response.len() < 2 || response[1] != pid {
            return Err(CoreError::Transport(format!(
                "Mode 01 response did not echo PID 0x{pid:02X}"
            )));
        }
        Ok(response[2..].to_vec())
    }

    /// Query the supported-PID masks and return the PIDs the vehicle reports.
    pub fn read_supported_pids(&mut self) -> Result<Vec<u8>> {
        let mut supported = Vec::new();
        let mut base = 0u8;
        loop {
            let data = self.read_pid(base)?;
            if data.len() < 4 {
                return Err(CoreError::Transport(format!(
                    "supported PID mask 0x{base:02X} is {} bytes",
                    data.len()
                )));
            }
            let mask = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
            for offset in 0..32u8 {
                if mask & (1 << (31 - offset)) != 0 {
                    supported.push(base.wrapping_add(offset + 1));
                }
            }
            let has_next_range = mask & 1 != 0;
            if !has_next_range || base >= 0x60 {
                break;
            }
            base = base.wrapping_add(0x20);
        }
        supported.sort_unstable();
        supported.dedup();
        Ok(supported)
    }

    pub fn read_vin(&mut self) -> Result<String> {
        let response = self.request(&[0x09, 0x02])?;
        let data = response.get(2..).unwrap_or_default();
        let vin: String = data
            .iter()
            .filter(|byte| byte.is_ascii_alphanumeric())
            .take(17)
            .map(|byte| *byte as char)
            .collect();
        if vin.len() != 17 {
            return Err(CoreError::Transport(format!(
                "VIN response contained {} usable characters",
                vin.len()
            )));
        }
        Ok(vin)
    }

    pub fn read_calibration_id(&mut self) -> Result<String> {
        let response = self.request(&[0x09, 0x04])?;
        let text = printable(&response[2..]);
        if text.is_empty() {
            return Err(CoreError::Transport(
                "calibration ID response was empty".into(),
            ));
        }
        Ok(text)
    }

    pub fn read_cvn(&mut self) -> Result<String> {
        let response = self.request(&[0x09, 0x06])?;
        let mut data = &response[2..];
        // Some ECUs prefix the data with a record count.
        if data.len() % 4 == 1 {
            data = &data[1..];
        }
        if data.is_empty() || data.len() % 4 != 0 {
            return Err(CoreError::Transport(format!(
                "unexpected CVN payload length {}",
                data.len()
            )));
        }
        Ok(data
            .chunks(4)
            .map(hex::encode_upper)
            .collect::<Vec<_>>()
            .join(" "))
    }

    pub fn read_mil_status(&mut self) -> Result<MilStatus> {
        let data = self.read_pid(0x01)?;
        if data.len() < 4 {
            return Err(CoreError::Transport(format!(
                "MIL status PID is {} bytes",
                data.len()
            )));
        }
        Ok(MilStatus {
            mil_on: data[0] & 0x80 != 0,
            dtc_count: data[0] & 0x7F,
            readiness: [data[1], data[2], data[3]],
        })
    }

    pub fn read_dtcs(&mut self, mode: DtcMode) -> Result<Vec<Dtc>> {
        let response = self.request(&[mode.service()])?;
        dtc::parse_response(mode.service(), &response)
    }

    pub fn clear_dtcs(&mut self) -> Result<()> {
        let response = self.request(&[0x04])?;
        if response.first().copied() != Some(0x44) {
            return Err(CoreError::Transport(
                "clear-DTC request was not accepted".into(),
            ));
        }
        Ok(())
    }
}

fn printable(bytes: &[u8]) -> String {
    bytes
        .iter()
        .filter(|byte| (0x20..=0x7E).contains(*byte))
        .map(|byte| *byte as char)
        .collect::<String>()
        .trim()
        .to_string()
}

pub fn nrc_message(code: u8) -> &'static str {
    match code {
        0x10 => "general reject",
        0x11 => "service not supported",
        0x12 => "sub-function, PID or format not supported",
        0x13 => "incorrect message length",
        0x14 => "response too long",
        0x21 => "busy, repeat request",
        0x22 => "conditions not correct",
        0x24 => "request sequence error",
        0x31 => "request out of range",
        0x33 => "security access denied",
        0x35 => "invalid key",
        0x36 => "exceeded number of attempts",
        0x37 => "required time delay not expired",
        0x70 => "upload or download not accepted",
        0x71 => "transfer data suspended",
        0x72 => "general programming failure",
        0x73 => "wrong block sequence counter",
        0x78 => "response pending",
        0x7E => "sub-function not supported in active session",
        0x7F => "service not supported in active session",
        _ => "unknown negative response code",
    }
}
