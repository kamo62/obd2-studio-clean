use crate::error::{CoreError, Result};
use crate::obd::dtc;
use crate::transport::DiagnosticTransport;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

const EMBEDDED_DEMO: &str = include_str!("../../../../fixtures/demo-vehicle.json");

/// A vehicle described entirely by a JSON fixture.
///
/// The fixture is intentionally data-only: PIDs are hex payloads, codes are
/// normal OBD-II strings and the VIN/calibration values are plain text. A
/// contributor can add a new demo vehicle without writing Rust.
#[derive(Debug, Deserialize)]
struct VehicleFixture {
    name: String,
    vin: String,
    calibration_id: String,
    cvn: String,
    pids: BTreeMap<String, String>,
    #[serde(default)]
    stored_codes: Vec<String>,
    #[serde(default)]
    pending_codes: Vec<String>,
    #[serde(default)]
    permanent_codes: Vec<String>,
}

pub struct FixtureVehicle {
    name: String,
    vin: String,
    calibration_id: String,
    cvn: Vec<u8>,
    pids: BTreeMap<u8, Vec<u8>>,
    stored_codes: Vec<[u8; 2]>,
    pending_codes: Vec<[u8; 2]>,
    permanent_codes: Vec<[u8; 2]>,
    cleared: bool,
}

impl FixtureVehicle {
    pub fn from_json(text: &str) -> Result<Self> {
        let fixture: VehicleFixture = serde_json::from_str(text)?;
        if fixture.vin.len() != 17 || !fixture.vin.chars().all(|ch| ch.is_ascii_alphanumeric()) {
            return Err(CoreError::Invalid(
                "fixture VIN must be 17 alphanumeric characters".into(),
            ));
        }
        let mut pids = BTreeMap::new();
        for (key, value) in &fixture.pids {
            let pid = u8::from_str_radix(key.trim_start_matches("0x"), 16)
                .map_err(|_| CoreError::Invalid(format!("fixture PID {key} is not hex")))?;
            let data = crate::util::decode_hex_strict(
                &value
                    .chars()
                    .filter(|ch| ch.is_ascii_hexdigit())
                    .collect::<String>(),
                4,
            )?;
            pids.insert(pid, data);
        }
        let cvn = crate::util::decode_hex_strict(&fixture.cvn, 4)?;
        if cvn.len() != 4 {
            return Err(CoreError::Invalid("fixture CVN must be 4 bytes".into()));
        }
        Ok(Self {
            name: fixture.name,
            vin: fixture.vin.to_uppercase(),
            calibration_id: fixture.calibration_id,
            cvn,
            pids,
            stored_codes: parse_codes(&fixture.stored_codes)?,
            pending_codes: parse_codes(&fixture.pending_codes)?,
            permanent_codes: parse_codes(&fixture.permanent_codes)?,
            cleared: false,
        })
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_json(&std::fs::read_to_string(path)?)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn vin(&self) -> &str {
        &self.vin
    }

    fn supported(&self, base: u8) -> [u8; 4] {
        let mut mask = 0u32;
        for pid in self.pids.keys() {
            if *pid > base && *pid <= base.saturating_add(0x20) {
                let offset = (*pid as u32) - (base as u32) - 1;
                mask |= 1 << (31 - offset);
            }
        }
        let has_next = self
            .pids
            .keys()
            .any(|pid| *pid > base.saturating_add(0x20) && *pid <= base.saturating_add(0x40));
        if has_next {
            mask |= 1;
        }
        mask.to_be_bytes()
    }

    fn pid(&self, pid: u8) -> Option<Vec<u8>> {
        let data = match pid {
            0x00 | 0x20 | 0x40 | 0x60 => self.supported(pid).to_vec(),
            0x01 => {
                let mut data = self
                    .pids
                    .get(&0x01)
                    .cloned()
                    .unwrap_or_else(|| vec![0, 0, 0, 0]);
                if self.cleared {
                    data[0] = 0;
                }
                data
            }
            _ => self.pids.get(&pid).cloned()?,
        };
        let mut response = vec![0x41, pid];
        response.extend_from_slice(&data);
        Some(response)
    }

    fn code_response(&self, service: u8, codes: &[[u8; 2]]) -> Vec<u8> {
        let mut response = vec![service.wrapping_add(0x40), codes.len() as u8];
        for code in codes {
            response.extend_from_slice(code);
        }
        response
    }
}

impl DiagnosticTransport for FixtureVehicle {
    fn request(&mut self, frame: &[u8], _timeout: Duration) -> Result<Vec<u8>> {
        let Some(&service) = frame.first() else {
            return Err(CoreError::Invalid("empty diagnostic request".into()));
        };
        match service {
            0x01 => {
                let pid = *frame
                    .get(1)
                    .ok_or_else(|| CoreError::Invalid("Mode 01 request has no PID".into()))?;
                self.pid(pid).ok_or_else(|| CoreError::NegativeResponse {
                    service,
                    code: 0x12,
                    message: format!("PID 0x{pid:02X} is not in the fixture"),
                })
            }
            0x03 => {
                let codes = if self.cleared {
                    &[][..]
                } else {
                    &self.stored_codes[..]
                };
                Ok(self.code_response(0x03, codes))
            }
            0x07 => {
                let codes = if self.cleared {
                    &[][..]
                } else {
                    &self.pending_codes[..]
                };
                Ok(self.code_response(0x07, codes))
            }
            0x0A => Ok(self.code_response(0x0A, &self.permanent_codes)),
            0x04 => {
                self.cleared = true;
                Ok(vec![0x44])
            }
            0x09 => match frame.get(1).copied() {
                Some(0x02) => {
                    let mut response = vec![0x49, 0x02, 0x01];
                    response.extend_from_slice(self.vin.as_bytes());
                    Ok(response)
                }
                Some(0x04) => {
                    let mut response = vec![0x49, 0x04, 0x01];
                    response.extend_from_slice(self.calibration_id.as_bytes());
                    response.push(0);
                    Ok(response)
                }
                Some(0x06) => {
                    let mut response = vec![0x49, 0x06, 0x01];
                    response.extend_from_slice(&self.cvn);
                    Ok(response)
                }
                Some(sub) => Err(CoreError::NegativeResponse {
                    service,
                    code: 0x12,
                    message: format!("Mode 09 info type 0x{sub:02X} is not in the fixture"),
                }),
                None => Err(CoreError::Invalid(
                    "Mode 09 request has no info type".into(),
                )),
            },
            0x10 => Err(CoreError::Unsupported(
                "fixture vehicles are OBD-II only; they have no UDS sessions".into(),
            )),
            0x3E => Ok(vec![0x7E, 0x00]),
            other => Err(CoreError::NegativeResponse {
                service,
                code: 0x11,
                message: format!("service 0x{other:02X} is not supported"),
            }),
        }
    }

    fn receive(&mut self, _timeout: Duration) -> Result<Vec<u8>> {
        Err(CoreError::Transport(
            "fixture vehicles have no pending response frames".into(),
        ))
    }
}

fn parse_codes(values: &[String]) -> Result<Vec<[u8; 2]>> {
    values.iter().map(|code| dtc::parse_code(code)).collect()
}

/// The deterministic vehicle used by the desktop demo mode.
///
/// Values come from [`fixtures/demo-vehicle.json`](../../../../fixtures/demo-vehicle.json):
/// 1726 rpm, 62 km/h, 88 °C coolant and P0133 plus P0420.
pub struct DemoVehicle {
    inner: FixtureVehicle,
}

impl Default for DemoVehicle {
    fn default() -> Self {
        Self::new()
    }
}

impl DemoVehicle {
    pub fn new() -> Self {
        Self {
            inner: FixtureVehicle::from_json(EMBEDDED_DEMO)
                .expect("the embedded demo vehicle fixture is valid"),
        }
    }

    pub fn name(&self) -> &str {
        self.inner.name()
    }
}

impl DiagnosticTransport for DemoVehicle {
    fn request(&mut self, frame: &[u8], timeout: Duration) -> Result<Vec<u8>> {
        self.inner.request(frame, timeout)
    }

    fn receive(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        self.inner.receive(timeout)
    }
}
