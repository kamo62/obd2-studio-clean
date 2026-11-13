use crate::error::{CoreError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dtc {
    pub code: String,
}

impl Dtc {
    pub fn system(&self) -> char {
        self.code.chars().next().unwrap_or('?')
    }
}

pub fn decode(bytes: [u8; 2]) -> Dtc {
    let letter = match bytes[0] >> 6 {
        0 => 'P',
        1 => 'C',
        2 => 'B',
        _ => 'U',
    };
    let code = format!(
        "{}{}{}{}{}",
        letter,
        (bytes[0] >> 4) & 0x03,
        bytes[0] & 0x0F,
        bytes[1] >> 4,
        bytes[1] & 0x0F
    );
    Dtc { code }
}

/// Parse codes such as `P0301` or `U0100` back into their two-byte form.
pub fn parse_code(text: &str) -> Result<[u8; 2]> {
    let bytes = text.as_bytes();
    if bytes.len() != 5 {
        return Err(CoreError::Invalid(format!(
            "DTC {text} must be five characters"
        )));
    }
    let system = match bytes[0].to_ascii_uppercase() {
        b'P' => 0x00,
        b'C' => 0x40,
        b'B' => 0x80,
        b'U' => 0xC0,
        _ => {
            return Err(CoreError::Invalid(format!(
                "DTC {text} must start with P, C, B or U"
            )));
        }
    };
    let first = hex_digit(bytes[1])?;
    if first > 3 {
        return Err(CoreError::Invalid(format!(
            "DTC {text} has an invalid first digit"
        )));
    }
    let second = hex_digit(bytes[2])?;
    let third = hex_digit(bytes[3])?;
    let fourth = hex_digit(bytes[4])?;
    Ok([system | (first << 4) | second, (third << 4) | fourth])
}

fn hex_digit(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(CoreError::Invalid("DTC contains a non-hex digit".into())),
    }
}

/// Parse the payload of Mode 03, 07 or 0A.
///
/// CAN responses carry a DTC count byte; K-line responses do not. An odd number
/// of bytes after the service identifier is treated as the CAN count form.
pub fn parse_response(service: u8, response: &[u8]) -> Result<Vec<Dtc>> {
    let expected = service.wrapping_add(0x40);
    if response.first().copied() != Some(expected) {
        return Err(CoreError::Invalid(format!(
            "expected response 0x{expected:02X}, got 0x{:02X}",
            response.first().copied().unwrap_or(0)
        )));
    }
    let mut data = &response[1..];
    if data.len() % 2 == 1 {
        data = &data[1..];
    }
    let mut codes = Vec::new();
    for pair in data.chunks_exact(2) {
        if pair == [0, 0] {
            continue;
        }
        codes.push(decode([pair[0], pair[1]]));
    }
    Ok(codes)
}
