use crate::coding::checksum::is_checksum_valid;
use crate::coding::types::{
    BackupBlock, BackupMetadata, BackupSummary, CafdRef, CodingBackup, CodingDataBlock,
    CodingEcuRef, FscFile, Integrity, NewBackup,
};
use crate::error::{CoreError, Result};
use crate::util::{
    decode_hex_strict, encode_hex_upper, sanitize_segment, sha256_hex, unique_suffix, write_atomic,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const FORMAT: &str = "obd2.coding-backup";
pub const VERSION: u32 = 1;
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_BLOCK_BYTES: usize = 65_535;
const MAX_BLOCKS: usize = 256;

#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub vin: Option<String>,
    pub ecu: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupStore {
    directory: PathBuf,
}

impl BackupStore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            directory: root.as_ref().join("backups"),
        }
    }

    pub fn with_directory(directory: impl AsRef<Path>) -> Self {
        Self {
            directory: directory.as_ref().to_path_buf(),
        }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn save(&self, input: NewBackup) -> Result<(PathBuf, CodingBackup)> {
        self.save_internal(input, true)
    }

    /// Save a recovery snapshot even when the live block checksum is damaged.
    pub fn save_snapshot(&self, input: NewBackup) -> Result<(PathBuf, CodingBackup)> {
        self.save_internal(input, false)
    }

    fn save_internal(
        &self,
        input: NewBackup,
        verify_checksums: bool,
    ) -> Result<(PathBuf, CodingBackup)> {
        let created = input.created_at.unwrap_or_else(Utc::now);
        let blocks = input
            .blocks
            .iter()
            .map(|block| BackupBlock {
                address: block.address,
                data: encode_hex_upper(&block.data),
                checksum_position: block.checksum_position,
                checksum_invert: block.checksum_invert,
                signature: block
                    .signature
                    .as_ref()
                    .map(|value| encode_hex_upper(value)),
            })
            .collect();

        let mut backup = CodingBackup {
            format: FORMAT.to_string(),
            version: VERSION,
            created_at: created.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            vin: input.vin.trim().to_uppercase(),
            ecu: input.ecu,
            cafd: input.cafd,
            blocks,
            cps: input.cps.as_ref().map(|value| encode_hex_upper(value)),
            fsc: input
                .fsc
                .iter()
                .map(|(name, data)| FscFile {
                    name: name.clone(),
                    data: encode_hex_upper(data),
                })
                .collect(),
            metadata: BackupMetadata {
                app_version: input.app_version,
                title: input.title,
                note: input.note,
            },
            integrity: Integrity {
                algorithm: "sha256".into(),
                digest: String::new(),
            },
        };
        validate_payload_with(&backup, verify_checksums)?;
        backup.integrity.digest = digest_of(&backup)?;

        let stamp = created.format("%Y%m%dT%H%M%S%3fZ").to_string();
        let name = format!(
            "{}-{}-{}-{}.cod",
            sanitize_segment(&backup.vin),
            sanitize_segment(&backup.ecu.identifier),
            stamp,
            unique_suffix()
        );
        let bytes = serde_json::to_vec_pretty(&backup)?;
        let path = write_atomic(&self.directory, &name, &bytes)?;
        Ok((path, backup))
    }

    pub fn load(&self, path: impl AsRef<Path>) -> Result<CodingBackup> {
        let path = path.as_ref();
        if path.extension().and_then(|value| value.to_str()) != Some("cod") {
            return Err(CoreError::Invalid("not a .cod backup".into()));
        }
        let metadata = fs::metadata(path)?;
        if metadata.len() > MAX_FILE_BYTES {
            return Err(CoreError::Invalid(format!(
                "backup file is {} bytes (limit {MAX_FILE_BYTES})",
                metadata.len()
            )));
        }
        let bytes = fs::read(path)?;
        let backup: CodingBackup = serde_json::from_slice(&bytes)?;
        // Storage integrity is schema + SHA-256. ECU checksums are enforced
        // when a restore is planned, so a damaged live block can still be
        // captured and later repaired.
        validate_payload_with(&backup, false)?;
        let expected = digest_of(&backup)?;
        if expected.to_lowercase() != backup.integrity.digest.to_lowercase() {
            return Err(CoreError::Integrity);
        }
        Ok(backup)
    }

    pub fn list(&self, filter: &ListFilter) -> Result<Vec<BackupSummary>> {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("cod") {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.len() > MAX_FILE_BYTES {
                continue;
            }
            let Ok(backup) = self.load(&path) else {
                continue;
            };
            if let Some(vin) = &filter.vin {
                if !backup.vin.eq_ignore_ascii_case(vin.trim()) {
                    continue;
                }
            }
            if let Some(ecu) = &filter.ecu {
                if !backup.ecu.identifier.eq_ignore_ascii_case(ecu.trim()) {
                    continue;
                }
            }
            out.push(BackupSummary {
                file: path.to_string_lossy().into_owned(),
                created_at: backup.created_at.clone(),
                vin: backup.vin.clone(),
                ecu: backup.ecu.clone(),
                cafd: backup.cafd.clone(),
                blocks: backup.blocks.len(),
                bytes: metadata.len(),
                title: backup.metadata.title.clone(),
            });
        }
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(out)
    }

    /// Delete by bare file name inside the store. Path components are rejected.
    pub fn delete_name(&self, name: &str) -> Result<()> {
        if Path::new(name).components().count() != 1 || name.contains('/') || name.contains('\\') {
            return Err(CoreError::Invalid(
                "backup name must not contain path components".into(),
            ));
        }
        if !name.ends_with(".cod") {
            return Err(CoreError::Invalid("not a .cod backup".into()));
        }
        let path = self.directory.join(name);
        fs::remove_file(path)?;
        Ok(())
    }

    /// Keep the newest `keep` backups for one VIN + ECU pair.
    pub fn prune(&self, vin: &str, ecu: &str, keep: usize) -> Result<Vec<String>> {
        if keep == 0 {
            return Err(CoreError::Invalid("keep must be at least 1".into()));
        }
        let filter = ListFilter {
            vin: Some(vin.into()),
            ecu: Some(ecu.into()),
        };
        let mut backups = self.list(&filter)?;
        // Recovery snapshots are never auto-pruned; they are the rollback set.
        backups.retain(|item| item.title.as_deref() != Some("Pre-restore snapshot"));
        backups.sort_by(|a, b| (&b.created_at, &b.file).cmp(&(&a.created_at, &a.file)));
        let mut removed = Vec::new();
        for backup in backups.into_iter().skip(keep) {
            let name = Path::new(&backup.file)
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| CoreError::Invalid("backup path has no file name".into()))?;
            self.delete_name(name)?;
            removed.push(name.to_string());
        }
        Ok(removed)
    }
}

fn digest_of(backup: &CodingBackup) -> Result<String> {
    let mut clone = backup.clone();
    clone.integrity.digest.clear();
    Ok(sha256_hex(&serde_json::to_vec(&clone)?))
}

pub fn validate_payload(backup: &CodingBackup) -> Result<()> {
    validate_payload_with(backup, true)
}

pub fn validate_payload_with(backup: &CodingBackup, verify_checksums: bool) -> Result<()> {
    if backup.format != FORMAT {
        return Err(CoreError::Invalid("wrong backup format".into()));
    }
    if backup.version != VERSION {
        return Err(CoreError::Invalid(format!(
            "unsupported backup version {}",
            backup.version
        )));
    }
    if backup.vin.trim().len() < 6 || backup.vin.len() > 64 {
        return Err(CoreError::Invalid("invalid VIN".into()));
    }
    if backup.ecu.identifier.trim().is_empty() {
        return Err(CoreError::Invalid("missing ECU identifier".into()));
    }
    if backup.ecu.address.trim().is_empty() {
        return Err(CoreError::Invalid("missing ECU address".into()));
    }
    if backup.cafd.identifier.trim().is_empty() || backup.cafd.data_version.trim().is_empty() {
        return Err(CoreError::Invalid("missing CAFD reference".into()));
    }
    if backup.metadata.app_version.trim().is_empty() {
        return Err(CoreError::Invalid("missing app version".into()));
    }
    if backup.integrity.algorithm != "sha256" {
        return Err(CoreError::Invalid("unsupported integrity algorithm".into()));
    }
    if backup.blocks.is_empty() || backup.blocks.len() > MAX_BLOCKS {
        return Err(CoreError::Invalid(format!(
            "block count must be 1..={MAX_BLOCKS}"
        )));
    }
    let mut seen = std::collections::HashSet::new();
    for block in &backup.blocks {
        if !seen.insert(block.address) {
            return Err(CoreError::Invalid(format!(
                "duplicate block address {}",
                block.address
            )));
        }
        let data = decode_hex_strict(&block.data, MAX_BLOCK_BYTES)?;
        if data.is_empty() {
            return Err(CoreError::Invalid(format!(
                "block {} is empty",
                block.address
            )));
        }
        if verify_checksums {
            if let Some(position) = block.checksum_position {
                if !is_checksum_valid(&data, position, block.checksum_invert)? {
                    return Err(CoreError::Invalid(format!(
                        "block {} checksum is invalid",
                        block.address
                    )));
                }
            }
        }
        if let Some(signature) = &block.signature {
            let decoded = decode_hex_strict(signature, 4096)?;
            if decoded.is_empty() {
                return Err(CoreError::Invalid(format!(
                    "block {} has an empty signature",
                    block.address
                )));
            }
        }
    }
    if let Some(cps) = &backup.cps {
        decode_hex_strict(cps, MAX_BLOCK_BYTES)?;
    }
    for fsc in &backup.fsc {
        if fsc.name.trim().is_empty() {
            return Err(CoreError::Invalid("FSC entry has no name".into()));
        }
        decode_hex_strict(&fsc.data, MAX_FILE_BYTES as usize)?;
    }
    Ok(())
}

pub fn decode_blocks(backup: &CodingBackup) -> Result<Vec<CodingDataBlock>> {
    backup
        .blocks
        .iter()
        .map(|block| {
            Ok(CodingDataBlock {
                address: block.address,
                data: decode_hex_strict(&block.data, MAX_BLOCK_BYTES)?,
                checksum_position: block.checksum_position,
                checksum_invert: block.checksum_invert,
                signature: block
                    .signature
                    .as_ref()
                    .map(|value| decode_hex_strict(value, 4096))
                    .transpose()?,
            })
        })
        .collect()
}

pub fn new_backup(
    vin: &str,
    ecu: CodingEcuRef,
    cafd: CafdRef,
    blocks: Vec<CodingDataBlock>,
    app_version: &str,
    created_at: Option<DateTime<Utc>>,
) -> NewBackup {
    NewBackup {
        vin: vin.into(),
        ecu,
        cafd,
        blocks,
        cps: None,
        fsc: Vec::new(),
        app_version: app_version.into(),
        title: None,
        note: None,
        created_at,
    }
}
