use obd2_core::DiagnosticTransport;
use obd2_core::coding::backup::BackupStore;
use obd2_core::coding::checksum::{is_checksum_valid, with_checksum};
use obd2_core::coding::restore::{ExecuteOptions, plan_restore};
use obd2_core::coding::types::{
    CafdRef, CodingDataBlock, CodingEcuRef, NewBackup, PlanOptions, RestoreTarget,
};
use obd2_core::obd::dtc;
use obd2_core::obd::session::{DtcMode, ObdSession};
use obd2_core::pack::{GenericPack, PackCatalog, PidValue};
use obd2_core::transport::demo::DemoVehicle;
use obd2_core::transport::elm327::{parse_response, strip_can_framing};
use obd2_core::transport::enet::EnetFrame;
use obd2_core::transport::simulator::SimulatorTransport;
use std::io::Cursor;
use std::path::Path;
use std::time::Duration;

fn workspace_path(relative: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

#[test]
fn generic_pack_loads_and_decodes_pids() {
    let pack = GenericPack::load(workspace_path("packs/generic-obd2/pack.json")).unwrap();
    assert_eq!(pack.pids.len(), 58);
    assert_eq!(pack.pid(0x05).unwrap().name, "Engine coolant temperature");

    let coolant = pack.decode(0x05, &[128]).unwrap();
    assert_eq!(coolant.display(), "88 °C");

    let rpm = pack.decode(0x0C, &[0x1A, 0xF8]).unwrap();
    match rpm {
        PidValue::Numeric { value, unit, .. } => {
            assert_eq!(value, 1726.0);
            assert_eq!(unit.as_deref(), Some("rpm"));
        }
        other => panic!("unexpected RPM value: {other:?}"),
    }

    let fuel = pack.decode(0x51, &[1]).unwrap();
    assert_eq!(fuel.display(), "Gasoline");
}

#[test]
fn pack_catalog_discovers_generic_and_bmw() {
    let catalog = PackCatalog::discover(workspace_path("packs")).unwrap();
    let ids: Vec<_> = catalog
        .packs()
        .iter()
        .map(|pack| pack.manifest.id.as_str())
        .collect();
    assert!(ids.contains(&"generic-obd2"));
    assert!(ids.contains(&"bmw-diagnostics"));
    assert!(ids.contains(&"bmw-coding"));
    assert!(catalog.generic("generic-obd2").is_ok());
}

#[test]
fn demo_vehicle_supports_the_whole_generic_flow() {
    let mut vehicle = DemoVehicle::new();
    let mut session = ObdSession::with_default_timeout(&mut vehicle);

    assert_eq!(session.read_vin().unwrap(), "WBA8E9C50GK123456");
    let pids = session.read_supported_pids().unwrap();
    assert!(pids.contains(&0x0C));
    assert!(pids.contains(&0x0D));
    assert!(!pids.contains(&0x02));

    let rpm = session.read_pid(0x0C).unwrap();
    assert_eq!(rpm, vec![0x1A, 0xF8]);

    let status = session.read_mil_status().unwrap();
    assert!(status.mil_on);
    assert_eq!(status.dtc_count, 2);

    let stored = session.read_dtcs(DtcMode::Stored).unwrap();
    let codes: Vec<_> = stored.iter().map(|code| code.code.as_str()).collect();
    assert_eq!(codes, ["P0133", "P0420"]);

    session.clear_dtcs().unwrap();
    assert!(session.read_dtcs(DtcMode::Stored).unwrap().is_empty());
    assert!(!session.read_mil_status().unwrap().mil_on);
}

#[test]
fn dtc_encoding_roundtrips() {
    for text in ["P0301", "C1234", "B0100", "U0100"] {
        let bytes = dtc::parse_code(text).unwrap();
        assert_eq!(dtc::decode(bytes).code, text);
    }
    assert!(dtc::parse_code("X0301").is_err());
    assert!(dtc::parse_code("P4301").is_err());

    let response = [0x43, 0x02, 0x01, 0x33, 0x04, 0x20];
    let codes = dtc::parse_response(0x03, &response).unwrap();
    assert_eq!(codes[0].code, "P0133");
    assert_eq!(codes[1].code, "P0420");
}

#[test]
fn elm327_response_parser_handles_echo_and_status_lines() {
    let parsed = parse_response("SEARCHING...\r41 0C 1A F8\r\r>").unwrap();
    assert_eq!(parsed, vec![0x41, 0x0C, 0x1A, 0xF8]);

    assert!(parse_response("SEARCHING...\rNO DATA\r\r>").is_err());
    assert!(parse_response("41 0C 1AF\r>").is_err());

    // A CAN header and a single-frame PCI byte are removed when present.
    assert_eq!(
        strip_can_framing(&[0x03, 0x41, 0x0C, 0x1A]).unwrap(),
        vec![0x41, 0x0C, 0x1A]
    );
}

struct Loopback {
    input: Cursor<Vec<u8>>,
    writes: Vec<u8>,
}

impl std::io::Read for Loopback {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.input.read(buffer)
    }
}

impl std::io::Write for Loopback {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.writes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn elm327_initializes_then_reads_a_pid() {
    // Six adapter configuration prompts, then one Mode 01 response.
    let input = concat!(
        "\rELM327 v1.5\r\r>",
        "\rOK\r\r>",
        "\rOK\r\r>",
        "\rOK\r\r>",
        "\rOK\r\r>",
        "\rOK\r\r>",
        "\rSEARCHING...\r41 0C 1A F8\r\r>"
    );
    let stream = Loopback {
        input: Cursor::new(input.as_bytes().to_vec()),
        writes: Vec::new(),
    };
    let mut adapter = obd2_core::transport::elm327::Elm327::new(stream);
    let response = adapter
        .request(&[0x01, 0x0C], Duration::from_secs(1))
        .unwrap();
    assert_eq!(response, vec![0x41, 0x0C, 0x1A, 0xF8]);
}

#[test]
fn simulator_replays_encoded_enet_frames() {
    let request = EnetFrame {
        control: 0x0001,
        source: 0xF4,
        target: 0x10,
        payload: vec![0x22, 0xF1, 0x90],
    };
    let response = EnetFrame {
        control: 0x0001,
        source: 0x10,
        target: 0xF4,
        payload: vec![0x62, 0xF1, 0x90, b'W', b'B', b'A'],
    };
    let fixture = serde_json::json!({
        "name": "unit-test",
        "frames": {
            hex::encode_upper(request.encode()): hex::encode_upper(response.encode())
        }
    });
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("frames.json");
    std::fs::write(&path, serde_json::to_vec(&fixture).unwrap()).unwrap();

    let mut simulator = SimulatorTransport::from_file(&path)
        .unwrap()
        .with_ecu("0xF4");
    assert_eq!(simulator.target_address(), Some(0xF4));

    let payload = simulator
        .request(&[0x22, 0xF1, 0x90], Duration::from_millis(100))
        .unwrap();
    assert_eq!(payload, vec![0x62, 0xF1, 0x90, b'W', b'B', b'A']);
}

fn ecu() -> CodingEcuRef {
    CodingEcuRef {
        address: "0x3020".into(),
        identifier: "0000000F".into(),
        identifiers: vec!["0000000F".into()],
        group: Some("G_AIRBAG".into()),
        name: Some("ACSM".into()),
    }
}

fn cafd() -> CafdRef {
    CafdRef {
        identifier: "0000000F".into(),
        data_version: "26.3.0".into(),
    }
}

fn block(address: u16, data: Vec<u8>) -> CodingDataBlock {
    CodingDataBlock {
        address,
        data,
        checksum_position: None,
        checksum_invert: false,
        signature: None,
    }
}

fn target() -> RestoreTarget {
    RestoreTarget {
        vin: "WBA8E9C50GK123456".into(),
        ecu_identifier: "0000000F".into(),
        ecu_address: "0x3020".into(),
        cafd_identifier: "0000000F".into(),
        data_version: "26.3.0".into(),
    }
}

#[test]
fn checksum_helpers_detect_damage() {
    let data = vec![0x10, 0x20, 0x30, 0x00];
    let sealed = with_checksum(&data, 3, false).unwrap();
    assert!(is_checksum_valid(&sealed, 3, false).unwrap());
    let mut damaged = sealed.clone();
    damaged[0] ^= 1;
    assert!(!is_checksum_valid(&damaged, 3, false).unwrap());
}

#[test]
fn every_pid_decodes_a_representative_payload() {
    let pack = GenericPack::load(workspace_path("packs/generic-obd2/pack.json")).unwrap();
    for pid in &pack.pids {
        let data = vec![0x40u8; pid.bytes as usize];
        let value = pack.decode(pid.pid, &data).unwrap_or_else(|error| {
            panic!(
                "PID 0x{:02X} ({}) failed to decode: {error}",
                pid.pid, pid.name
            )
        });
        assert!(!value.display().is_empty());
    }
}

#[test]
fn demo_fixture_can_be_loaded_from_disk() {
    let vehicle = obd2_core::transport::demo::FixtureVehicle::from_file(workspace_path(
        "fixtures/demo-vehicle.json",
    ))
    .unwrap();
    assert_eq!(vehicle.name(), "Demo vehicle");
    assert_eq!(vehicle.vin(), "WBA8E9C50GK123456");
}

#[test]
fn coding_backup_roundtrips_and_rejects_tampering() {
    let directory = tempfile::tempdir().unwrap();
    let store = BackupStore::with_directory(directory.path());
    let backup = NewBackup {
        vin: "WBA8E9C50GK123456".into(),
        ecu: ecu(),
        cafd: cafd(),
        blocks: vec![block(0x1000, vec![1, 2, 3, 4]), block(0x1001, vec![5, 6])],
        cps: None,
        fsc: Vec::new(),
        app_version: "0.1.0".into(),
        title: Some("Unit test".into()),
        note: None,
        created_at: None,
    };
    let (path, saved) = store.save(backup).unwrap();
    assert_eq!(saved.vin, "WBA8E9C50GK123456");
    assert_eq!(store.list(&Default::default()).unwrap().len(), 1);

    let plan = plan_restore(&saved, &target(), &PlanOptions::default()).unwrap();
    assert_eq!(plan.blocks.len(), 2);

    let mut text = std::fs::read_to_string(&path).unwrap();
    text = text.replace("\"digest\": \"", "\"digest\": \"0");
    std::fs::write(&path, text).unwrap();
    assert!(store.load(&path).is_err());
}

#[test]
fn restore_plan_rejects_wrong_vehicle() {
    let directory = tempfile::tempdir().unwrap();
    let store = BackupStore::with_directory(directory.path());
    let backup = NewBackup {
        vin: "WBA8E9C50GK123456".into(),
        ecu: ecu(),
        cafd: cafd(),
        blocks: vec![block(0x1000, vec![1, 2, 3, 4])],
        cps: None,
        fsc: Vec::new(),
        app_version: "0.1.0".into(),
        title: None,
        note: None,
        created_at: None,
    };
    let (_, saved) = store.save(backup).unwrap();
    let mut wrong = target();
    wrong.vin = "WBA8E9C50GK999999".into();
    let error = plan_restore(&saved, &wrong, &PlanOptions::default()).unwrap_err();
    assert!(error.to_string().contains("VIN"));
}

#[test]
fn bmw_packs_open_when_present() {
    let diagnostics = workspace_path("packs/bmw-diagnostics/bmw-diagnostics.sqlite");
    if diagnostics.is_file() {
        let pack = obd2_core::pack::BmwDiagnosticsPack::open(&diagnostics).unwrap();
        let meta = pack.meta().unwrap();
        assert_eq!(
            meta.get("source").map(String::as_str),
            Some("BMW vehicle diagnostic dataset")
        );
        assert!(pack.ecus().unwrap().len() > 800);
        assert!(pack.variants().unwrap().len() > 1000);
        assert!(
            pack.fault_text("0x027100", "error", "en")
                .unwrap()
                .is_some()
        );
        assert!(!pack.sensors("dme8ff_r", "en").unwrap().is_empty());
        assert!(
            !pack
                .faults_for_ecu("aag_02", Some("error"), "en", 5)
                .unwrap()
                .is_empty()
        );
        assert!(!pack.search_faults("0x027100", "en", 5).unwrap().is_empty());
    }

    let coding = workspace_path("packs/bmw-coding/bmw-coding.sqlite");
    if coding.is_file() {
        let pack = obd2_core::pack::BmwCodingPack::open(&coding).unwrap();
        assert!(pack.data_version().unwrap().is_some());
        assert!(!pack.sections().unwrap().is_empty());
        assert!(pack.expert(None, 5).unwrap().len() <= 5);
        assert!(
            !pack
                .expert(Some("START_STOP_FUNCTION"), 10)
                .unwrap()
                .is_empty()
        );
        let detail = pack.expert_entry(1).unwrap().unwrap();
        assert!(!detail.options.is_empty());
        assert!(
            !pack
                .function_writes("TC_MSA_DEFAULT_OFF", 5)
                .unwrap()
                .is_empty()
        );
        assert!(!pack.cafd_files(3).unwrap().is_empty());
    }
}

#[test]
fn execute_options_are_sane() {
    let options = ExecuteOptions::default();
    assert!(options.verify);
    assert!(options.verify_live_vin);
    assert_eq!(options.timeout, Duration::from_secs(2));
}

// Keep the Cursor import exercised by a small compile-time assertion: the
// generic ELM327 stream accepts any Read + Write implementation.
#[allow(dead_code)]
fn elm327_accepts_in_memory_stream() {
    let _ = obd2_core::transport::elm327::Elm327::new(Cursor::new(Vec::new()));
}
