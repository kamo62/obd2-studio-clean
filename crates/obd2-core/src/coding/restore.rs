use crate::coding::backup::{BackupStore, decode_blocks};
use crate::coding::checksum::is_checksum_valid;
use crate::coding::types::NewBackup;
use crate::coding::types::{
    BlockAddress, BlockOutcome, CodingBackup, CodingDataBlock, PlanOptions, RestoreOutcome,
    RestorePlan, RestoreTarget,
};
use crate::error::{CoreError, Result};
use crate::transport::{DiagnosticTransport, SecurityAccess};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct ExecuteOptions {
    pub verify: bool,
    /// Read the live VIN (UDS 22 F1 90) and require it to match the backup.
    pub verify_live_vin: bool,
    pub timeout: Duration,
    /// Overall P2* window for a 0x78 response.
    pub p2_star: Duration,
    pub max_pending: usize,
    pub close_session: bool,
}

impl Default for ExecuteOptions {
    fn default() -> Self {
        Self {
            verify: true,
            verify_live_vin: true,
            timeout: Duration::from_secs(2),
            p2_star: Duration::from_secs(10),
            max_pending: 64,
            close_session: true,
        }
    }
}

fn problems_to_error(problems: Vec<String>) -> CoreError {
    CoreError::Restore(problems.join("; "))
}

fn addresses_equal(left: &str, right: &str) -> bool {
    match (parse_hex_address(left), parse_hex_address(right)) {
        (Ok(a), Ok(b)) => a == b,
        _ => left.eq_ignore_ascii_case(right),
    }
}

fn parse_hex_address(value: &str) -> Result<u16> {
    let digits = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    u16::from_str_radix(digits, 16)
        .map_err(|_| CoreError::Restore(format!("invalid ECU address {value}")))
}

/// Validate a backup against the module it is about to be written to.
///
/// Every identity field is required. There is no "unknown target" path.
pub fn plan_restore(
    backup: &CodingBackup,
    target: &RestoreTarget,
    options: &PlanOptions,
) -> Result<RestorePlan> {
    let mut problems = Vec::new();
    let mut warnings = Vec::new();

    if !backup.vin.eq_ignore_ascii_case(target.vin.trim()) {
        problems.push(format!("VIN {} != backup {}", target.vin, backup.vin));
    }
    if !backup
        .ecu
        .identifier
        .eq_ignore_ascii_case(target.ecu_identifier.trim())
    {
        problems.push(format!(
            "ECU identifier {} != backup {}",
            target.ecu_identifier, backup.ecu.identifier
        ));
    }
    if !addresses_equal(&backup.ecu.address, target.ecu_address.trim()) {
        problems.push(format!(
            "ECU address {} != backup {}",
            target.ecu_address, backup.ecu.address
        ));
    }
    if !backup
        .cafd
        .identifier
        .eq_ignore_ascii_case(target.cafd_identifier.trim())
    {
        problems.push(format!(
            "CAFD {} != backup {}",
            target.cafd_identifier, backup.cafd.identifier
        ));
    }
    if backup.cafd.data_version != target.data_version {
        let message = format!(
            "CAFD data version {} != backup {}",
            target.data_version, backup.cafd.data_version
        );
        if options.allow_data_version_mismatch {
            warnings.push(message);
        } else {
            problems.push(message);
        }
    }
    if backup.cps.is_some() || !backup.fsc.is_empty() {
        return Err(CoreError::Unsupported(
            "backup contains CPS or FSC data; that restore path is not implemented".into(),
        ));
    }
    if options.require_signature {
        return Err(CoreError::Unsupported(
            "signature verification/application is not implemented".into(),
        ));
    }
    if backup.blocks.iter().any(|block| block.signature.is_some()) {
        // Silently dropping a signature would produce a different block than the
        // one that was backed up.
        return Err(CoreError::Unsupported(
            "signature-bearing blocks cannot be restored yet".into(),
        ));
    }

    let blocks = decode_blocks(backup)?;
    if blocks.is_empty() {
        problems.push("backup has no blocks".into());
    }
    for block in &blocks {
        if options.verify_checksums {
            if let Some(position) = block.checksum_position {
                match is_checksum_valid(&block.data, position, block.checksum_invert) {
                    Ok(true) => {}
                    Ok(false) => {
                        problems.push(format!("block {} checksum is invalid", block.address))
                    }
                    Err(error) => problems.push(error.to_string()),
                }
            }
        }
    }

    if !problems.is_empty() {
        return Err(problems_to_error(problems));
    }
    Ok(RestorePlan {
        backup: backup.clone(),
        blocks,
        warnings,
    })
}

fn frame(sid: u8, address: BlockAddress, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(3 + data.len());
    out.extend_from_slice(&[sid, (address >> 8) as u8, address as u8]);
    out.extend_from_slice(data);
    out
}

fn require_positive(response: &[u8], sid: u8, what: &str) -> Result<BlockAddress> {
    if response.len() >= 3 && response[0] == sid.wrapping_add(0x40) {
        let address = ((response[1] as u16) << 8) | response[2] as u16;
        return Ok(address);
    }
    if response.len() >= 3 && response[0] == 0x7f {
        return Err(CoreError::Transport(format!(
            "{what} rejected with NRC 0x{:02x}",
            response[2]
        )));
    }
    Err(CoreError::Transport(format!(
        "unexpected {what} response {}",
        hex::encode_upper(response)
    )))
}

fn wait_for_final(
    transport: &mut dyn DiagnosticTransport,
    sid: u8,
    first: Vec<u8>,
    options: &ExecuteOptions,
) -> Result<Vec<u8>> {
    let mut response = first;
    let mut deadline = Instant::now() + options.p2_star;
    let mut pending = 0usize;
    loop {
        if response.len() >= 3 && response[0] == 0x7f && response[1] == sid && response[2] == 0x78 {
            pending += 1;
            if pending > options.max_pending || Instant::now() >= deadline {
                return Err(CoreError::Transport("0x78 continuation timed out".into()));
            }
            // Each pending response restarts the P2* window; the request is
            // never resent.
            deadline = Instant::now() + options.p2_star;
            response = transport.receive(options.p2_star)?;
            continue;
        }
        return Ok(response);
    }
}

fn write_block(
    transport: &mut dyn DiagnosticTransport,
    address: BlockAddress,
    data: &[u8],
    options: &ExecuteOptions,
) -> Result<()> {
    let first = transport.request(&frame(0x2e, address, data), options.timeout)?;
    let response = wait_for_final(transport, 0x2e, first, options)?;
    let echoed = require_positive(&response, 0x2e, "write")?;
    if echoed != address {
        return Err(CoreError::Transport(format!(
            "write ack DID 0x{echoed:04x} != 0x{address:04x}"
        )));
    }
    Ok(())
}

fn read_block(
    transport: &mut dyn DiagnosticTransport,
    address: BlockAddress,
    options: &ExecuteOptions,
) -> Result<Vec<u8>> {
    let first = transport.request(&frame(0x22, address, &[]), options.timeout)?;
    let response = wait_for_final(transport, 0x22, first, options)?;
    let echoed = require_positive(&response, 0x22, "read")?;
    if echoed != address {
        return Err(CoreError::Transport(format!(
            "read ack DID 0x{echoed:04x} != 0x{address:04x}"
        )));
    }
    Ok(response[3..].to_vec())
}

/// Read the live VIN (UDS 22 F1 90).
pub fn read_vin(
    transport: &mut dyn DiagnosticTransport,
    options: &ExecuteOptions,
) -> Result<String> {
    let first = transport.request(&[0x22, 0xf1, 0x90], options.timeout)?;
    let response = wait_for_final(transport, 0x22, first, options)?;
    let did = require_positive(&response, 0x22, "VIN read")?;
    if did != 0xf190 {
        return Err(CoreError::Transport(format!(
            "VIN response echoed DID 0x{did:04x}, expected 0xf190"
        )));
    }
    let data = &response[3..];
    let end = data
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(data.len());
    let raw = &data[..end];
    if raw.iter().any(|byte| !byte.is_ascii_alphanumeric()) {
        return Err(CoreError::Transport(
            "VIN contains non-alphanumeric bytes".into(),
        ));
    }
    if raw.len() != 17 {
        return Err(CoreError::Transport(format!(
            "VIN length {} is not 17",
            raw.len()
        )));
    }
    Ok(String::from_utf8_lossy(raw).to_uppercase())
}

fn snapshot_before_write(
    plan: &RestorePlan,
    transport: &mut dyn DiagnosticTransport,
    store: &BackupStore,
    options: &ExecuteOptions,
    app_version: &str,
) -> Result<std::path::PathBuf> {
    let mut blocks = Vec::with_capacity(plan.blocks.len());
    for block in &plan.blocks {
        let current = read_block(transport, block.address, options)?;
        blocks.push(CodingDataBlock {
            address: block.address,
            data: current,
            // A damaged block is exactly what a restore may need to repair;
            // do not let checksum validation prevent the recovery snapshot.
            checksum_position: block.checksum_position,
            checksum_invert: block.checksum_invert,
            signature: None,
        });
    }
    let (path, _) = store.save_snapshot(NewBackup {
        vin: plan.backup.vin.clone(),
        ecu: plan.backup.ecu.clone(),
        cafd: plan.backup.cafd.clone(),
        blocks,
        cps: None,
        fsc: Vec::new(),
        app_version: app_version.to_string(),
        title: Some("Pre-restore snapshot".into()),
        note: Some("Automatically captured before a restore write".into()),
        created_at: None,
    })?;
    Ok(path)
}

/// Resolve a capture's VIN and ECU address against the live identity.
///
/// The live VIN is authoritative; a non-empty supplied VIN must match it.
/// When the transport reports a target, the supplied address must match it and
/// the stored address is derived from the target.
pub fn resolve_capture_identity(
    live_vin: &str,
    supplied_vin: &str,
    transport_target: Option<u16>,
    supplied_address: &str,
) -> Result<(String, String)> {
    if !supplied_vin.trim().is_empty() && !supplied_vin.eq_ignore_ascii_case(live_vin) {
        return Err(CoreError::Restore(format!(
            "supplied VIN {supplied_vin} != live VIN {live_vin}"
        )));
    }
    let address = match transport_target {
        Some(actual) => {
            if supplied_address.trim().is_empty() {
                format_address(actual)
            } else {
                let supplied = parse_hex_address(supplied_address)?;
                if supplied != actual {
                    return Err(CoreError::Restore(format!(
                        "supplied ECU address 0x{supplied:04X} != transport target 0x{actual:04X}"
                    )));
                }
                format_address(actual)
            }
        }
        None => {
            if supplied_address.trim().is_empty() {
                return Err(CoreError::Restore("ECU address is required".into()));
            }
            supplied_address.trim().to_string()
        }
    };
    Ok((live_vin.to_uppercase(), address))
}

fn format_address(value: u16) -> String {
    if value <= 0xff {
        format!("0x{value:02X}")
    } else {
        format!("0x{value:04X}")
    }
}

/// Read the live VIN and the requested blocks, then close the session.
pub fn capture_session(
    transport: &mut dyn DiagnosticTransport,
    addresses: &[BlockAddress],
    options: &ExecuteOptions,
) -> Result<(String, Vec<CodingDataBlock>)> {
    let first = transport.request(&[0x10, 0x03], options.timeout)?;
    let session = wait_for_final(transport, 0x10, first, options)?;
    if !(session.len() >= 2 && session[0] == 0x50 && session[1] == 0x03) {
        return Err(CoreError::Transport(format!(
            "session rejected: {}",
            hex::encode_upper(&session)
        )));
    }
    let result = (|| {
        let vin = read_vin(transport, options)?;
        let mut blocks = Vec::with_capacity(addresses.len());
        for address in addresses {
            let data = read_block(transport, *address, options)?;
            blocks.push(CodingDataBlock {
                address: *address,
                data,
                checksum_position: None,
                checksum_invert: false,
                signature: None,
            });
        }
        Ok((vin, blocks))
    })();
    if options.close_session {
        let _ = transport.request(&[0x10, 0x01], options.timeout);
    }
    result
}

/// Write a validated plan. A recovery snapshot of the current blocks is taken
/// before the first write; the first failed block aborts the run.
pub fn execute_restore(
    plan: &RestorePlan,
    transport: &mut dyn DiagnosticTransport,
    mut security: Option<&mut dyn SecurityAccess>,
    options: &ExecuteOptions,
    recovery: &BackupStore,
    app_version: &str,
) -> Result<RestoreOutcome> {
    // Identity is checked before the module is even put into a session.
    if let Some(transport_target) = transport.target_address() {
        let expected = parse_hex_address(&plan.backup.ecu.address)?;
        if transport_target != expected {
            return Err(CoreError::Restore(format!(
                "transport target 0x{transport_target:04x} != backup ECU address 0x{expected:04x}"
            )));
        }
    }

    let first = transport.request(&[0x10, 0x03], options.timeout)?;
    let session = wait_for_final(transport, 0x10, first, options)?;
    if !(session.len() >= 2 && session[0] == 0x50 && session[1] == 0x03) {
        return Err(CoreError::Transport(format!(
            "session rejected: {}",
            hex::encode_upper(&session)
        )));
    }

    let close = |transport: &mut dyn DiagnosticTransport| {
        if options.close_session {
            let _ = transport.request(&[0x10, 0x01], options.timeout);
        }
    };

    if options.verify_live_vin {
        match read_vin(transport, options) {
            Ok(vin) => {
                if !vin.eq_ignore_ascii_case(&plan.backup.vin) {
                    close(transport);
                    return Err(CoreError::Restore(format!(
                        "live VIN {vin} != backup {}",
                        plan.backup.vin
                    )));
                }
            }
            Err(error) => {
                close(transport);
                return Err(error);
            }
        }
    }

    if let Some(access) = security.as_mut() {
        if let Err(error) = access.unlock(transport, &plan.backup.ecu) {
            close(transport);
            return Err(error);
        }
    }

    let snapshot = match snapshot_before_write(plan, transport, recovery, options, app_version) {
        Ok(path) => Some(path),
        Err(error) => {
            close(transport);
            return Err(error);
        }
    };

    let mut completed = Vec::new();
    let mut verified = Vec::new();
    let mut failed = None;
    let mut index = 0usize;

    while index < plan.blocks.len() {
        let block = &plan.blocks[index];
        let written = match write_block(transport, block.address, &block.data, options) {
            Ok(()) => true,
            Err(error) => {
                failed = Some(BlockOutcome {
                    address: block.address,
                    written: false,
                    verified: false,
                    error: Some(error.to_string()),
                });
                break;
            }
        };
        if options.verify {
            match read_block(transport, block.address, options) {
                Ok(current) if current == block.data => {
                    verified.push(block.address);
                }
                Ok(_) => {
                    failed = Some(BlockOutcome {
                        address: block.address,
                        written,
                        verified: false,
                        error: Some("read-back differs from written block".into()),
                    });
                    break;
                }
                Err(error) => {
                    failed = Some(BlockOutcome {
                        address: block.address,
                        written,
                        verified: false,
                        error: Some(error.to_string()),
                    });
                    break;
                }
            }
        }
        completed.push(block.address);
        index += 1;
    }

    let unattempted: Vec<BlockAddress> = plan.blocks[index + usize::from(failed.is_some())..]
        .iter()
        .map(|block| block.address)
        .collect();
    close(transport);
    Ok(RestoreOutcome {
        completed,
        failed,
        unattempted,
        verified,
        snapshot,
    })
}
