use crate::coding::types::CodingEcuRef;
use crate::error::Result;
use std::time::Duration;

pub mod demo;
pub mod doip;
pub mod elm327;
pub mod enet;
pub mod simulator;

/// Byte-level diagnostic transport.
///
/// Implementations own framing (ELM327, ENET, DoIP) and return the diagnostic
/// payload only. Addresses are tester/ECU addresses where the protocol has
/// them; a plain OBD-II adapter can return `None`.
pub trait DiagnosticTransport {
    fn request(&mut self, frame: &[u8], timeout: Duration) -> Result<Vec<u8>>;

    /// Receive a follow-up frame without resending the request. Used for the
    /// UDS `0x78` response-pending flow.
    fn receive(&mut self, timeout: Duration) -> Result<Vec<u8>>;

    /// The diagnostic target this transport is opened against, when known.
    fn target_address(&self) -> Option<u16> {
        None
    }
}

/// Optional security access for modules that require an unlock before writes.
///
/// The core does not implement a key algorithm. Applications and vehicle packs
/// can provide one once it has been bench-validated.
pub trait SecurityAccess {
    fn unlock(&mut self, transport: &mut dyn DiagnosticTransport, ecu: &CodingEcuRef)
    -> Result<()>;
}
