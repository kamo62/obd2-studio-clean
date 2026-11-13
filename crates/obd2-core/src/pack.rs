use crate::error::{CoreError, Result};
use crate::obd::pid::{Formula, format_number};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// The manifest every pack carries in `pack.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackManifest {
    pub schema: u32,
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub brand: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub data_version: Option<String>,
    #[serde(default)]
    pub database: Option<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub protocols: Vec<String>,
    #[serde(default)]
    pub coverage: BTreeMap<String, u64>,
    #[serde(default)]
    pub notes: Option<String>,
}

impl PackManifest {
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

#[derive(Debug, Clone)]
pub struct LoadedPack {
    pub manifest: PackManifest,
    pub directory: PathBuf,
    pub database: Option<PathBuf>,
}

/// Discover packs below a directory. A pack is any direct child directory with
/// a readable `pack.json`.
#[derive(Debug, Clone, Default)]
pub struct PackCatalog {
    packs: Vec<LoadedPack>,
}

impl PackCatalog {
    pub fn discover(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let metadata = match fs::symlink_metadata(root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(CoreError::Invalid(
                "pack root must be a real directory".into(),
            ));
        }
        let root = root.canonicalize()?;
        let entries = fs::read_dir(&root)?;
        let mut packs = Vec::new();
        for entry in entries.flatten() {
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let directory = entry.path();
            let manifest_path = directory.join("pack.json");
            match fs::symlink_metadata(&manifest_path) {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
                Ok(_) => {
                    return Err(CoreError::Invalid(
                        "pack manifest must be a regular file".into(),
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            }
            let bytes = fs::read(&manifest_path)?;
            let manifest: PackManifest = serde_json::from_slice(&bytes).map_err(|error| {
                CoreError::Invalid(format!("{}: {error}", manifest_path.display()))
            })?;
            let database = match manifest.database.as_deref() {
                None => None,
                Some(name) => {
                    let mut components = Path::new(name).components();
                    if !matches!(components.next(), Some(Component::Normal(_)))
                        || components.next().is_some()
                        || name.contains(['/', '\\'])
                    {
                        return Err(CoreError::Invalid(
                            "pack database must be a bare file name".into(),
                        ));
                    }
                    let path = directory.join(name);
                    match fs::symlink_metadata(&path) {
                        Ok(metadata)
                            if metadata.is_file() && !metadata.file_type().is_symlink() =>
                        {
                            let path = path.canonicalize()?;
                            if !path.starts_with(&directory) {
                                return Err(CoreError::Invalid(
                                    "pack database escaped its directory".into(),
                                ));
                            }
                            Some(path)
                        }
                        Ok(_) => {
                            return Err(CoreError::Invalid(
                                "pack database must be a regular file".into(),
                            ));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                        Err(error) => return Err(error.into()),
                    }
                }
            };
            packs.push(LoadedPack {
                manifest,
                directory,
                database,
            });
        }
        packs.sort_by(|left, right| left.manifest.id.cmp(&right.manifest.id));
        Ok(Self { packs })
    }

    pub fn packs(&self) -> &[LoadedPack] {
        &self.packs
    }

    pub fn get(&self, id: &str) -> Option<&LoadedPack> {
        self.packs.iter().find(|pack| pack.manifest.id == id)
    }

    pub fn generic(&self, id: &str) -> Result<GenericPack> {
        let pack = self
            .get(id)
            .ok_or_else(|| CoreError::Invalid(format!("pack {id} is not installed")))?;
        GenericPack::load(pack.directory.join("pack.json"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PidDecode {
    Numeric,
    Enum,
    Bitmask,
    Text,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PidDefinition {
    pub pid: u8,
    pub name: String,
    pub bytes: u8,
    pub decode: PidDecode,
    #[serde(default)]
    pub formula: Option<String>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub values: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DtcSystem {
    pub letter: String,
    pub name: String,
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenericPack {
    pub schema: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub scope: Option<String>,
    pub version: String,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub sources: Vec<String>,
    pub pids: Vec<PidDefinition>,
    #[serde(default)]
    pub dtc_systems: Vec<DtcSystem>,
    #[serde(default)]
    pub dtc_descriptions: HashMap<String, String>,
}

impl GenericPack {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = fs::read(path)?;
        let pack: Self = serde_json::from_slice(&bytes)?;
        pack.validate()?;
        Ok(pack)
    }

    pub fn from_json(text: &str) -> Result<Self> {
        let pack: Self = serde_json::from_str(text)?;
        pack.validate()?;
        Ok(pack)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != 1 {
            return Err(CoreError::Invalid(format!(
                "pack {} uses unsupported schema {}",
                self.id, self.schema
            )));
        }
        let mut seen = std::collections::HashSet::new();
        for pid in &self.pids {
            if !seen.insert(pid.pid) {
                return Err(CoreError::Invalid(format!(
                    "pack {} defines PID 0x{:02X} twice",
                    self.id, pid.pid
                )));
            }
            if !(1..=4).contains(&pid.bytes) {
                return Err(CoreError::Invalid(format!(
                    "PID 0x{:02X} has {} bytes; expected 1 to 4",
                    pid.pid, pid.bytes
                )));
            }
            if pid.name.trim().is_empty() {
                return Err(CoreError::Invalid(format!(
                    "PID 0x{:02X} has no name",
                    pid.pid
                )));
            }
            match pid.decode {
                PidDecode::Numeric => {
                    let formula = pid.formula.as_deref().ok_or_else(|| {
                        CoreError::Invalid(format!("PID 0x{:02X} has no formula", pid.pid))
                    })?;
                    Formula::parse(formula)?;
                }
                PidDecode::Enum if pid.values.is_empty() => {
                    return Err(CoreError::Invalid(format!(
                        "PID 0x{:02X} is an enum without values",
                        pid.pid
                    )));
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn pid(&self, pid: u8) -> Option<&PidDefinition> {
        self.pids.iter().find(|definition| definition.pid == pid)
    }

    pub fn category(&self, category: &str) -> impl Iterator<Item = &PidDefinition> {
        self.pids
            .iter()
            .filter(move |pid| pid.category.as_deref() == Some(category))
    }

    pub fn decode(&self, pid: u8, data: &[u8]) -> Result<PidValue> {
        let definition = self
            .pid(pid)
            .ok_or_else(|| CoreError::Unsupported(format!("PID 0x{pid:02X} is not in the pack")))?;
        decode_pid(definition, data)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PidValue {
    Numeric {
        value: f64,
        unit: Option<String>,
        text: String,
    },
    Labels(Vec<String>),
    Bits {
        hex: String,
        set: Vec<u8>,
        labels: Vec<String>,
    },
    Text(String),
}

impl PidValue {
    pub fn display(&self) -> String {
        match self {
            Self::Numeric { text, unit, .. } => match unit {
                Some(unit) if !unit.is_empty() => format!("{text} {unit}"),
                _ => text.clone(),
            },
            Self::Labels(labels) => labels.join(" / "),
            Self::Bits { hex, labels, .. } => {
                if labels.is_empty() {
                    hex.clone()
                } else {
                    format!("{hex} ({})", labels.join(", "))
                }
            }
            Self::Text(text) => text.clone(),
        }
    }
}

pub fn decode_pid(definition: &PidDefinition, data: &[u8]) -> Result<PidValue> {
    if data.len() < definition.bytes as usize {
        return Err(CoreError::Invalid(format!(
            "PID 0x{:02X} has {} byte(s), needs {}",
            definition.pid,
            data.len(),
            definition.bytes
        )));
    }
    let data = &data[..definition.bytes as usize];
    match definition.decode {
        PidDecode::Numeric => {
            let formula = Formula::parse(
                definition
                    .formula
                    .as_deref()
                    .ok_or_else(|| CoreError::Invalid("missing PID formula".into()))?,
            )?;
            let value = formula.evaluate(data)?;
            Ok(PidValue::Numeric {
                value,
                unit: definition.unit.clone(),
                text: format_number(value),
            })
        }
        PidDecode::Enum => {
            let mut labels = Vec::new();
            for (index, byte) in data.iter().enumerate() {
                if let Some(label) = definition.values.get(&byte.to_string()) {
                    if data.len() > 1 {
                        labels.push(format!("{}: {label}", index + 1));
                    } else {
                        labels.push(label.clone());
                    }
                }
            }
            if labels.is_empty() {
                labels.push(crate::util::hex_bytes(data));
            }
            Ok(PidValue::Labels(labels))
        }
        PidDecode::Bitmask => {
            let mut value = 0u32;
            for byte in data {
                value = (value << 8) | *byte as u32;
            }
            let bits = data.len() as u32 * 8;
            let mut set = Vec::new();
            let mut labels = Vec::new();
            for bit in 0..bits {
                if value & (1 << (bits - 1 - bit)) != 0 {
                    set.push(bit as u8);
                    if let Some(label) = definition.values.get(&bit.to_string()) {
                        labels.push(label.clone());
                    }
                }
            }
            Ok(PidValue::Bits {
                hex: crate::util::hex_bytes(data),
                set,
                labels,
            })
        }
        PidDecode::Text => Ok(PidValue::Text(
            data.iter()
                .filter(|byte| (0x20..=0x7E).contains(*byte))
                .map(|byte| *byte as char)
                .collect::<String>()
                .trim()
                .to_string(),
        )),
    }
}

// ---------------------------------------------------------------------------
// BMW diagnostics pack
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct EcuInfo {
    pub name: String,
    pub source_file: String,
    pub error_count: i64,
    pub info_count: i64,
    pub sensor_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EcuVariant {
    pub uds: bool,
    pub address: String,
    pub group_name: Option<String>,
    pub description_file: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SensorDefinition {
    pub sensor_id: Option<String>,
    pub arg: Option<String>,
    pub label: Option<String>,
    pub result_name: Option<String>,
    pub data_type: Option<String>,
    pub mul: Option<f64>,
    pub div: Option<f64>,
    pub offset: Option<f64>,
    pub start_index: Option<i64>,
    pub unit: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FaultCode {
    pub ecu: String,
    pub code: String,
    pub kind: String,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FaultText {
    pub code: String,
    pub kind: String,
    pub text: String,
}

pub struct BmwDiagnosticsPack {
    conn: Connection,
}

impl BmwDiagnosticsPack {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(Self { conn })
    }

    pub fn meta(&self) -> Result<BTreeMap<String, String>> {
        let mut stmt = self.conn.prepare("SELECT key, value FROM meta")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn ecus(&self) -> Result<Vec<EcuInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, source_file, error_count, info_count, sensor_count
             FROM ecus ORDER BY name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(EcuInfo {
                name: row.get(0)?,
                source_file: row.get(1)?,
                error_count: row.get(2)?,
                info_count: row.get(3)?,
                sensor_count: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn ecu(&self, name: &str) -> Result<Option<EcuInfo>> {
        Ok(self
            .conn
            .query_row(
                "SELECT name, source_file, error_count, info_count, sensor_count
                 FROM ecus WHERE name = ?1",
                params![name],
                |row| {
                    Ok(EcuInfo {
                        name: row.get(0)?,
                        source_file: row.get(1)?,
                        error_count: row.get(2)?,
                        info_count: row.get(3)?,
                        sensor_count: row.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn variants(&self) -> Result<Vec<EcuVariant>> {
        let mut stmt = self.conn.prepare(
            "SELECT uds, address, group_name, description_file
             FROM ecu_variants ORDER BY address",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(EcuVariant {
                uds: row.get::<_, i64>(0)? != 0,
                address: row.get(1)?,
                group_name: row.get(2)?,
                description_file: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn variant_for_ecu(&self, ecu: &str) -> Result<Option<EcuVariant>> {
        Ok(self
            .conn
            .query_row(
                "SELECT uds, address, group_name, description_file
                 FROM ecu_variants WHERE description_file = ?1 LIMIT 1",
                params![ecu],
                |row| {
                    Ok(EcuVariant {
                        uds: row.get::<_, i64>(0)? != 0,
                        address: row.get(1)?,
                        group_name: row.get(2)?,
                        description_file: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn fault_text(&self, code: &str, kind: &str, lang: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT text FROM fault_dictionary
                 WHERE code = ?1 AND kind = ?2 AND lang = ?3",
                params![code, kind, lang],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn faults_for_ecu(
        &self,
        ecu: &str,
        kind: Option<&str>,
        lang: &str,
        limit: u32,
    ) -> Result<Vec<FaultCode>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.ecu, f.code, f.kind, d.text
             FROM fault_codes f
             LEFT JOIN fault_dictionary d
               ON d.code = f.code AND d.kind = f.kind AND d.lang = ?3
             WHERE f.ecu = ?1 AND (?2 IS NULL OR f.kind = ?2)
             ORDER BY f.code LIMIT ?4",
        )?;
        let rows = stmt.query_map(params![ecu, kind, lang, limit], |row| {
            Ok(FaultCode {
                ecu: row.get(0)?,
                code: row.get(1)?,
                kind: row.get(2)?,
                text: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn search_faults(&self, query: &str, lang: &str, limit: u32) -> Result<Vec<FaultText>> {
        let needle = format!("%{}%", query.trim());
        let mut stmt = self.conn.prepare(
            "SELECT code, kind, text FROM fault_dictionary
             WHERE lang = ?1 AND (code LIKE ?2 OR text LIKE ?2)
             ORDER BY code LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![lang, needle, limit], |row| {
            Ok(FaultText {
                code: row.get(0)?,
                kind: row.get(1)?,
                text: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn sensors(&self, ecu: &str, lang: &str) -> Result<Vec<SensorDefinition>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.sensor_id, s.arg, COALESCE(n.name, s.label), s.result_name,
                    s.data_type, s.mul, s.div, s.offset_value, s.start_index, s.unit
             FROM sensors s
             LEFT JOIN sensor_names n
               ON n.ecu = s.ecu AND n.sensor_id IS s.sensor_id
              AND n.arg IS s.arg AND n.start_index IS s.start_index AND n.lang = ?2
             WHERE s.ecu = ?1
             ORDER BY s.sensor_id, s.start_index",
        )?;
        let rows = stmt.query_map(params![ecu, lang], |row| {
            Ok(SensorDefinition {
                sensor_id: row.get(0)?,
                arg: row.get(1)?,
                label: row.get(2)?,
                result_name: row.get(3)?,
                data_type: row.get(4)?,
                mul: row.get(5)?,
                div: row.get(6)?,
                offset: row.get(7)?,
                start_index: row.get(8)?,
                unit: row.get(9)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn salapa_text(&self, code: &str, lang: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT text FROM salapa_texts WHERE code = ?1 AND lang = ?2",
                params![code, lang],
                |row| row.get(0),
            )
            .optional()?)
    }
}

// ---------------------------------------------------------------------------
// BMW coding pack
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ExpertEntry {
    pub id: i64,
    pub file: String,
    pub section: Option<String>,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExpertFunction {
    pub name: String,
    pub values: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExpertOption {
    pub name: String,
    pub functions: Vec<ExpertFunction>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExpertEntryDetail {
    pub entry: ExpertEntry,
    pub options: Vec<ExpertOption>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FunctionWrite {
    pub file: String,
    pub group: String,
    pub address: String,
    pub name: String,
    pub start: String,
    pub end: String,
    pub mask: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CafdFile {
    pub filename: String,
    pub groups: i64,
    pub functions: i64,
}

pub struct BmwCodingPack {
    conn: Connection,
}

impl BmwCodingPack {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(Self { conn })
    }

    pub fn meta(&self) -> Result<BTreeMap<String, String>> {
        let mut stmt = self.conn.prepare("SELECT key, value FROM meta")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn data_version(&self) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key='data_version'",
                [],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn sections(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT COALESCE(section, 'OTHER'), COUNT(*) FROM expert_entries
             GROUP BY COALESCE(section, 'OTHER') ORDER BY 2 DESC, 1",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn expert(&self, section: Option<&str>, limit: u32) -> Result<Vec<ExpertEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, file, section, name, description_text
             FROM expert_entries
             WHERE (?1 IS NULL OR section = ?1)
             ORDER BY section, name LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![section, limit], |row| {
            Ok(ExpertEntry {
                id: row.get(0)?,
                file: row.get(1)?,
                section: row.get(2)?,
                name: row.get(3)?,
                description: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn search_expert(&self, query: &str, limit: u32) -> Result<Vec<ExpertEntry>> {
        let needle = format!("%{}%", query.trim());
        let mut stmt = self.conn.prepare(
            "SELECT id, file, section, name, description_text
             FROM expert_entries
             WHERE name LIKE ?1 OR description_text LIKE ?1
             ORDER BY section, name LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![needle, limit], |row| {
            Ok(ExpertEntry {
                id: row.get(0)?,
                file: row.get(1)?,
                section: row.get(2)?,
                name: row.get(3)?,
                description: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn expert_entry(&self, id: i64) -> Result<Option<ExpertEntryDetail>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, file, section, name, description_text
             FROM expert_entries WHERE id = ?1",
        )?;
        let entry = stmt
            .query_row(params![id], |row| {
                Ok(ExpertEntry {
                    id: row.get(0)?,
                    file: row.get(1)?,
                    section: row.get(2)?,
                    name: row.get(3)?,
                    description: row.get(4)?,
                })
            })
            .optional()?;
        let Some(entry) = entry else { return Ok(None) };

        let mut stmt = self.conn.prepare(
            "SELECT o.id, o.value_name, f.function_name, f.values_json
             FROM expert_options o
             LEFT JOIN expert_option_functions f ON f.option_id = o.id
             WHERE o.entry_id = ?1 ORDER BY o.id, f.id",
        )?;
        let mut rows = stmt.query(params![id])?;
        let mut options: Vec<ExpertOption> = Vec::new();
        let mut current_id: Option<i64> = None;
        while let Some(row) = rows.next()? {
            let option_id: i64 = row.get(0)?;
            let value_name: String = row.get(1)?;
            if current_id != Some(option_id) {
                current_id = Some(option_id);
                options.push(ExpertOption {
                    name: value_name,
                    functions: Vec::new(),
                });
            }
            let function_name: Option<String> = row.get(2)?;
            if let Some(name) = function_name {
                let raw: Option<String> = row.get(3)?;
                let values = match raw.as_deref() {
                    Some(value) => serde_json::from_str(value)?,
                    None => serde_json::Value::Array(Vec::new()),
                };
                if let Some(option) = options.last_mut() {
                    option.functions.push(ExpertFunction { name, values });
                }
            }
        }
        Ok(Some(ExpertEntryDetail { entry, options }))
    }

    pub fn function_writes(&self, name: &str, limit: u32) -> Result<Vec<FunctionWrite>> {
        let mut stmt = self.conn.prepare(
            "SELECT g.file, g.name, g.address, f.name, f.start, f.end, f.mask
             FROM coding_functions f JOIN data_groups g ON g.id = f.group_id
             WHERE f.name = ?1 ORDER BY g.file, g.address LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![name, limit], |row| {
            Ok(FunctionWrite {
                file: row.get(0)?,
                group: row.get(1)?,
                address: row.get(2)?,
                name: row.get(3)?,
                start: row.get(4)?,
                end: row.get(5)?,
                mask: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn cafd_files(&self, limit: u32) -> Result<Vec<CafdFile>> {
        let mut stmt = self.conn.prepare(
            "SELECT filename, groups, functions FROM cafd_files
             ORDER BY filename LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok(CafdFile {
                filename: row.get(0)?,
                groups: row.get(1)?,
                functions: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}
