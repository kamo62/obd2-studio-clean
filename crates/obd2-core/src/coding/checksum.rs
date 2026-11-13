use crate::error::{CoreError, Result};

/// XOR every byte except the checksum byte, optionally inverted.
///
/// Reversed from the BMW coding-block checksum used by the reference app.
pub fn xor_checksum(data: &[u8], checksum_position: usize, invert: bool) -> Result<u8> {
    if checksum_position >= data.len() {
        return Err(CoreError::Invalid(format!(
            "checksum position {checksum_position} outside block length {}",
            data.len()
        )));
    }
    let mut value = 0u8;
    for (index, byte) in data.iter().enumerate() {
        if index != checksum_position {
            value ^= byte;
        }
    }
    Ok(if invert { !value } else { value })
}

pub fn with_checksum(data: &[u8], checksum_position: usize, invert: bool) -> Result<Vec<u8>> {
    let value = xor_checksum(data, checksum_position, invert)?;
    let mut out = data.to_vec();
    out[checksum_position] = value;
    Ok(out)
}

pub fn is_checksum_valid(data: &[u8], checksum_position: usize, invert: bool) -> Result<bool> {
    Ok(
        data.get(checksum_position).copied()
            == Some(xor_checksum(data, checksum_position, invert)?),
    )
}
