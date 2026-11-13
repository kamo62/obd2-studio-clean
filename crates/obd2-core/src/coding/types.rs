use serde::{Deserialize, Serialize};

pub type BlockAddress = u16;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CodingEcuRef {
    /// Diagnostic address, e.g. "0x3020" or "00 0F1050".
    pub address: String,
    /// CAFD / ECU identifier, e.g. "0000000F".
    pub identifier: String,
    #[serde(default)]
    pub identifiers: Vec<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CafdRef {
    pub identifier: String,
    pub data_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FscFile {
    pub name: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackupMetadata {
    pub app_version: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Integrity {
    pub algorithm: String,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackupBlock {
    pub address: BlockAddress,
    pub data: String,
    #[serde(default)]
    pub checksum_position: Option<usize>,
    #[serde(default)]
    pub checksum_invert: bool,
    #[serde(default)]
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CodingBackup {
    pub format: String,
    pub version: u32,
    pub created_at: String,
    pub vin: String,
    pub ecu: CodingEcuRef,
    pub cafd: CafdRef,
    pub blocks: Vec<BackupBlock>,
    #[serde(default)]
    pub cps: Option<String>,
    #[serde(default)]
    pub fsc: Vec<FscFile>,
    pub metadata: BackupMetadata,
    pub integrity: Integrity,
}

/// In-memory block used when creating a backup or planning a restore.
#[derive(Debug, Clone)]
pub struct CodingDataBlock {
    pub address: BlockAddress,
    pub data: Vec<u8>,
    pub checksum_position: Option<usize>,
    pub checksum_invert: bool,
    pub signature: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct NewBackup {
    pub vin: String,
    pub ecu: CodingEcuRef,
    pub cafd: CafdRef,
    pub blocks: Vec<CodingDataBlock>,
    pub cps: Option<Vec<u8>>,
    pub fsc: Vec<(String, Vec<u8>)>,
    pub app_version: String,
    pub title: Option<String>,
    pub note: Option<String>,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupSummary {
    pub file: String,
    pub created_at: String,
    pub vin: String,
    pub ecu: CodingEcuRef,
    pub cafd: CafdRef,
    pub blocks: usize,
    pub bytes: u64,
    pub title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RestoreTarget {
    pub vin: String,
    pub ecu_identifier: String,
    /// Required: a restore must name the module address it is written to.
    pub ecu_address: String,
    pub cafd_identifier: String,
    pub data_version: String,
}

#[derive(Debug, Clone)]
pub struct PlanOptions {
    pub require_signature: bool,
    pub verify_checksums: bool,
    pub allow_data_version_mismatch: bool,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            require_signature: false,
            verify_checksums: true,
            allow_data_version_mismatch: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RestorePlan {
    pub backup: CodingBackup,
    pub blocks: Vec<CodingDataBlock>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BlockOutcome {
    pub address: BlockAddress,
    pub written: bool,
    pub verified: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RestoreOutcome {
    pub completed: Vec<BlockAddress>,
    pub failed: Option<BlockOutcome>,
    pub unattempted: Vec<BlockAddress>,
    pub verified: Vec<BlockAddress>,
    /// Recovery snapshot written before the first block.
    pub snapshot: Option<std::path::PathBuf>,
}
