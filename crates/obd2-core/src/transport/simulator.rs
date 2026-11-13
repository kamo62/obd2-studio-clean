use crate::error::{CoreError, Result};
use crate::transport::DiagnosticTransport;
use crate::transport::enet::EnetFrame;
use crate::util::{decode_hex_strict, encode_hex_upper};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct FixtureFile {
    name: String,
    frames: HashMap<String, String>,
}

/// Replay recorded frames.
///
/// Two formats are supported:
/// - ENET/HSFZ recordings (`sim-enet.json`): decoded to `(target, UDS request)`
///   -> UDS response, with synthesized session/write fallbacks.
/// - Legacy DDC recordings (`{ECU}...` keys): exact-match text only; these are
///   not UDS and cannot drive a restore.
pub struct SimulatorTransport {
    pub name: String,
    frames: HashMap<String, String>,
    uds: HashMap<(u8, String), String>,
    global: HashMap<String, String>,
    context_label: Option<String>,
    context_u16: Option<u16>,
}

impl SimulatorTransport {
    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let bytes = fs::read(path)?;
        let fixture: FixtureFile = serde_json::from_slice(&bytes)?;
        let frames: HashMap<String, String> = fixture
            .frames
            .into_iter()
            .map(|(key, value)| (key.to_uppercase(), value))
            .collect();

        let mut uds = HashMap::new();
        let mut global = HashMap::new();
        let mut enet = !frames.is_empty();
        for (key, value) in &frames {
            let Ok(key_bytes) = hex::decode(key) else {
                enet = false;
                break;
            };
            let Ok(value_bytes) = hex::decode(value) else {
                enet = false;
                break;
            };
            let (Ok(request), Ok(response)) = (
                EnetFrame::decode(&key_bytes),
                EnetFrame::decode(&value_bytes),
            ) else {
                enet = false;
                break;
            };
            let request_hex = encode_hex_upper(&request.payload);
            let response_hex = encode_hex_upper(&response.payload);
            uds.insert((request.target, request_hex.clone()), response_hex.clone());
            // Only the gateway VIN is safe to serve across targets. Block data
            // is ECU-specific and must not fall back to another target.
            if request_hex == "22F190" {
                global.entry(request_hex).or_insert(response_hex);
            }
        }

        if enet {
            // Session control is often absent from a read-only capture.
            global
                .entry("1003".into())
                .or_insert_with(|| "500300".into());
            global.entry("1001".into()).or_insert_with(|| "5001".into());
            global.entry("3E00".into()).or_insert_with(|| "7E00".into());
        }

        Ok(Self {
            name: fixture.name,
            frames,
            uds,
            global,
            context_label: None,
            context_u16: None,
        })
    }

    /// Restrict lookups to one ECU: `{XX}` in DDC fixtures, target address in ENET.
    pub fn with_ecu(mut self, ecu: &str) -> Self {
        self.context_label = Some(ecu.trim().trim_start_matches("0x").to_uppercase());
        self.context_u16 = parse_hex(ecu);
        self
    }

    pub fn lookup_hex(&self, request: &str) -> Result<String> {
        let needle = request
            .chars()
            .filter(|ch| ch.is_ascii_hexdigit())
            .collect::<String>()
            .to_uppercase();

        if !self.uds.is_empty() {
            let target = self.context_u16.and_then(|value| u8::try_from(value).ok());
            if let Some(target) = target {
                if let Some(value) = self.uds.get(&(target, needle.clone())) {
                    return Ok(value.clone());
                }
            }
            if let Some(value) = self.global.get(&needle) {
                return Ok(value.clone());
            }
            // A capture may not contain the write/echo path. Synthesize only
            // the deterministic UDS acknowledgements needed to exercise it.
            if let Some(rest) = needle.strip_prefix("2E") {
                if rest.len() >= 4 {
                    return Ok(format!("6E{}", &rest[..4]));
                }
            }
            return Err(CoreError::Transport(format!(
                "no UDS fixture for {needle} in {}",
                self.name
            )));
        }

        if let Some(context) = &self.context_label {
            let key = format!("{{{context}}}{needle}");
            return self.frames.get(&key).cloned().ok_or_else(|| {
                CoreError::Transport(format!("no fixture for {key} in {}", self.name))
            });
        }
        if let Some(value) = self.frames.get(&needle) {
            return Ok(value.clone());
        }
        Err(CoreError::Transport(format!(
            "no fixture for {needle} in {}",
            self.name
        )))
    }
}

fn parse_hex(value: &str) -> Option<u16> {
    u16::from_str_radix(
        value
            .trim()
            .trim_start_matches("0x")
            .trim_start_matches("0X"),
        16,
    )
    .ok()
}

impl DiagnosticTransport for SimulatorTransport {
    fn request(&mut self, frame: &[u8], _timeout: Duration) -> Result<Vec<u8>> {
        let request = encode_hex_upper(frame);
        let response = self.lookup_hex(&request)?;
        let normalized: String = response
            .chars()
            .filter(|ch| ch.is_ascii_hexdigit())
            .collect();
        if normalized.len() % 2 != 0 {
            return Err(CoreError::Transport(format!(
                "fixture frame for {request} is nibble-aligned; not UDS"
            )));
        }
        decode_hex_strict(&normalized, 65_535)
    }

    fn receive(&mut self, _timeout: Duration) -> Result<Vec<u8>> {
        Err(CoreError::Transport(
            "recorded fixtures do not model response-pending frames".into(),
        ))
    }

    fn target_address(&self) -> Option<u16> {
        self.context_u16
    }
}
