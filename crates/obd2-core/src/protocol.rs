pub fn read_data_by_identifier(id: u16) -> Vec<u8> {
    vec![0x22, (id >> 8) as u8, id as u8]
}

pub fn write_data_by_identifier(id: u16, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(3 + data.len());
    out.extend_from_slice(&[0x2e, (id >> 8) as u8, id as u8]);
    out.extend_from_slice(data);
    out
}

pub fn security_access(level: u8) -> Vec<u8> {
    vec![0x27, level]
}

pub fn tester_present() -> Vec<u8> {
    vec![0x3e, 0x00]
}

pub fn diagnostic_session(mode: u8) -> Vec<u8> {
    vec![0x10, mode]
}

pub fn is_positive(response: &[u8], request_sid: u8) -> bool {
    response.first().copied() == Some(request_sid.wrapping_add(0x40))
}

pub fn negative_response(response: &[u8]) -> Option<(u8, u8)> {
    if response.len() >= 3 && response[0] == 0x7f {
        Some((response[1], response[2]))
    } else {
        None
    }
}
