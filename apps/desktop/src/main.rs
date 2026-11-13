mod ui;

use gpui::{
    App as GpuiApp, Application, Bounds, ClickEvent, Context, Entity, FontWeight, Render,
    SharedString, Window, WindowBounds, WindowOptions, div, prelude::*, px, rgb, size,
};
use obd2_core::obd::session::{DtcMode, MilStatus, ObdSession};
use obd2_core::pack::{
    BmwCodingPack, BmwDiagnosticsPack, ExpertEntry, ExpertEntryDetail, FaultText, GenericPack,
    PackCatalog,
};
use obd2_core::transport::demo::DemoVehicle;
use obd2_core::transport::doip::DoipClient;
use obd2_core::transport::elm327::Elm327;
use obd2_core::transport::enet::EnetClient;
use obd2_core::{CoreError, DiagnosticTransport, Result};
use std::path::PathBuf;
use std::time::Duration;
use ui::text_field::TextField;

const CANVAS: u32 = 0x0b1118;
const HEADER: u32 = 0x111923;
const PANEL: u32 = 0x16222e;
const FIELD: u32 = 0x111923;
const BORDER: u32 = 0x344557;
const BORDER_STRONG: u32 = 0x49657d;
const TEXT: u32 = 0xe7eef5;
const MUTED: u32 = 0x9eafbe;
const ACCENT: u32 = 0x2d9cdb;
const ACCENT_DARK: u32 = 0x183247;
const SUCCESS: u32 = 0x56d98b;
const WARNING: u32 = 0xf0b85a;
const ERROR: u32 = 0xf2777a;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Connect,
    Live,
    Codes,
    Vehicle,
    Packs,
    Coding,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Adapter {
    Demo,
    Elm327,
    Enet,
    Doip,
}

#[derive(Clone, Copy)]
enum Tone {
    Neutral,
    Good,
    Warning,
    Error,
}

struct LiveRow {
    pid: u8,
    name: String,
    value: String,
}

enum LiveTransport {
    Demo(DemoVehicle),
    Elm(Elm327<std::net::TcpStream>),
    Enet(EnetClient),
    Doip(DoipClient),
}

impl DiagnosticTransport for LiveTransport {
    fn target_address(&self) -> Option<u16> {
        match self {
            Self::Demo(_) => None,
            Self::Elm(_) => None,
            Self::Enet(value) => value.target_address(),
            Self::Doip(value) => value.target_address(),
        }
    }

    fn request(&mut self, frame: &[u8], timeout: Duration) -> Result<Vec<u8>> {
        match self {
            Self::Demo(value) => value.request(frame, timeout),
            Self::Elm(value) => value.request(frame, timeout),
            Self::Enet(value) => value.request(frame, timeout),
            Self::Doip(value) => value.request(frame, timeout),
        }
    }

    fn receive(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        match self {
            Self::Demo(value) => value.receive(timeout),
            Self::Elm(value) => value.receive(timeout),
            Self::Enet(value) => value.receive(timeout),
            Self::Doip(value) => value.receive(timeout),
        }
    }
}

struct App {
    tab: Tab,
    adapter: Adapter,
    address: Entity<TextField>,
    source: Entity<TextField>,
    target: Entity<TextField>,
    fault_query: Entity<TextField>,

    transport: Option<LiveTransport>,
    message: String,
    tone: Tone,

    live: Vec<LiveRow>,
    mil: Option<MilStatus>,
    stored: Vec<String>,
    pending: Vec<String>,
    permanent: Vec<String>,

    vin: String,
    calibration: String,
    cvn: String,
    supported_count: usize,

    packs_root: PathBuf,
    catalog: PackCatalog,
    generic: Option<GenericPack>,
    diagnostics: Option<BmwDiagnosticsPack>,
    coding: Option<BmwCodingPack>,
    selected_pack: Option<String>,
    fault_results: Vec<FaultText>,

    coding_sections: Vec<(String, i64)>,
    coding_section: Option<String>,
    coding_entries: Vec<ExpertEntry>,
    coding_detail: Option<ExpertEntryDetail>,
}

impl App {
    fn new(cx: &mut Context<Self>) -> Self {
        let packs_root = PathBuf::from("packs");
        let catalog = PackCatalog::discover(&packs_root).unwrap_or_default();
        let generic = catalog.generic("generic-obd2").ok();
        let diagnostics = None;
        let coding = None;
        let coding_sections = Vec::new();
        let (message, tone) = if catalog.packs().is_empty() {
            (
                format!("No packs found in {}", packs_root.display()),
                Tone::Warning,
            )
        } else {
            (
                format!("{} pack(s) loaded", catalog.packs().len()),
                Tone::Neutral,
            )
        };

        Self {
            tab: Tab::Connect,
            adapter: Adapter::Demo,
            address: field(cx, "192.168.0.10:35000", "Adapter address"),
            source: field(cx, "0xF4", "Tester address"),
            target: field(cx, "0x01", "Target address"),
            fault_query: field(cx, "", "Code or fault text"),
            transport: None,
            message,
            tone,
            live: Vec::new(),
            mil: None,
            stored: Vec::new(),
            pending: Vec::new(),
            permanent: Vec::new(),
            vin: String::new(),
            calibration: String::new(),
            cvn: String::new(),
            supported_count: 0,
            packs_root,
            catalog,
            generic,
            diagnostics,
            coding,
            selected_pack: None,
            fault_results: Vec::new(),
            coding_sections,
            coding_section: None,
            coding_entries: Vec::new(),
            coding_detail: None,
        }
    }

    fn value(&self, field: &Entity<TextField>, cx: &Context<Self>) -> String {
        field.read(cx).value()
    }

    fn fail(&mut self, context: &str, error: impl std::fmt::Display) {
        self.message = format!("{context}: {error}");
        self.tone = Tone::Error;
    }

    fn clear_session_data(&mut self) {
        self.transport = None;
        self.live.clear();
        self.mil = None;
        self.stored.clear();
        self.pending.clear();
        self.permanent.clear();
        self.vin.clear();
        self.calibration.clear();
        self.cvn.clear();
        self.supported_count = 0;
    }

    fn connect(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let address = self.value(&self.address, cx);
        let source = self.value(&self.source, cx);
        let target = self.value(&self.target, cx);
        let timeout = Duration::from_secs(3);
        self.clear_session_data();

        let result = match self.adapter {
            Adapter::Demo => Ok(LiveTransport::Demo(DemoVehicle::new())),
            Adapter::Elm327 => Elm327::connect_tcp(&address, timeout).map(LiveTransport::Elm),
            Adapter::Enet => parse_u8(&source).and_then(|source| {
                parse_u8(&target).and_then(|target| {
                    EnetClient::connect(&address, source, target, timeout).map(LiveTransport::Enet)
                })
            }),
            Adapter::Doip => parse_u16(&source).and_then(|source| {
                parse_u16(&target).and_then(|target| {
                    DoipClient::connect(&address, source, target, timeout).and_then(|mut client| {
                        client.activate(timeout)?;
                        Ok(LiveTransport::Doip(client))
                    })
                })
            }),
        };

        match result {
            Ok(transport) => {
                self.transport = Some(transport);
                self.message = format!("Connected via {}", self.adapter.label());
                self.tone = Tone::Good;
                self.scan_identity();
                if !self.vin.is_empty() {
                    self.message =
                        format!("Connected via {} · VIN {}", self.adapter.label(), self.vin);
                }
            }
            Err(error) => {
                self.transport = None;
                self.fail("Connection failed", error);
            }
        }
        cx.notify();
    }

    fn disconnect(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.clear_session_data();
        self.message = "Disconnected".into();
        self.tone = Tone::Warning;
        cx.notify();
    }

    /// Read identity fields without treating a missing optional value as a
    /// connection failure.
    fn scan_identity(&mut self) {
        let Some(transport) = self.transport.as_mut() else {
            return;
        };
        let mut session = ObdSession::with_default_timeout(transport);
        if let Ok(vin) = session.read_vin() {
            self.vin = vin;
        }
        if let Ok(calibration) = session.read_calibration_id() {
            self.calibration = calibration;
        }
        if let Ok(cvn) = session.read_cvn() {
            self.cvn = cvn;
        }
        if let Ok(pids) = session.read_supported_pids() {
            self.supported_count = pids.len();
        }
    }

    fn scan_vehicle(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.transport.is_none() {
            self.message = "Connect an adapter first".into();
            self.tone = Tone::Warning;
            cx.notify();
            return;
        }
        self.scan_identity();
        if let Some(mil) = self.read_mil() {
            self.mil = Some(mil);
        }
        self.message = if self.vin.is_empty() {
            "Vehicle scanned; the ECU did not report a VIN".into()
        } else {
            format!("Vehicle scanned · VIN {}", self.vin)
        };
        self.tone = Tone::Good;
        cx.notify();
    }

    fn read_mil(&mut self) -> Option<MilStatus> {
        let transport = self.transport.as_mut()?;
        let mut session = ObdSession::with_default_timeout(transport);
        session.read_mil_status().ok()
    }

    fn refresh_live(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let result: Result<(Vec<LiveRow>, usize, usize)> = (|| {
            let pack = self.generic.as_ref().ok_or_else(|| {
                CoreError::Unsupported("the generic-obd2 pack is not installed".into())
            })?;
            let transport = self
                .transport
                .as_mut()
                .ok_or_else(|| CoreError::Transport("not connected".into()))?;
            let mut session = ObdSession::with_default_timeout(transport);
            let supported = session.read_supported_pids()?;
            let mut rows = Vec::new();
            let mut failures = 0;
            for pid in &supported {
                let Some(definition) = pack.pid(*pid) else {
                    continue;
                };
                match session
                    .read_pid(*pid)
                    .and_then(|data| pack.decode(*pid, &data))
                {
                    Ok(value) => rows.push(LiveRow {
                        pid: *pid,
                        name: definition.name.clone(),
                        value: value.display(),
                    }),
                    Err(_) => failures += 1,
                }
            }
            Ok((rows, supported.len(), failures))
        })();

        match result {
            Ok((rows, supported, failures)) => {
                let count = rows.len();
                self.live = rows;
                self.supported_count = supported;
                self.message = if failures == 0 {
                    format!("Read {count} live value(s) from {supported} supported PID(s)")
                } else {
                    format!("Read {count} live value(s); {failures} PID(s) did not answer")
                };
                self.tone = Tone::Good;
            }
            Err(error) => self.fail("Live data failed", error),
        }
        cx.notify();
    }

    fn read_codes(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let result: Result<(MilStatus, Vec<String>, Vec<String>, Vec<String>)> = (|| {
            let transport = self
                .transport
                .as_mut()
                .ok_or_else(|| CoreError::Transport("not connected".into()))?;
            let mut session = ObdSession::with_default_timeout(transport);
            let mil = session.read_mil_status()?;
            let stored = codes(session.read_dtcs(DtcMode::Stored)?);
            let pending = codes(session.read_dtcs(DtcMode::Pending)?);
            let permanent = codes(session.read_dtcs(DtcMode::Permanent)?);
            Ok((mil, stored, pending, permanent))
        })();

        match result {
            Ok((mil, stored, pending, permanent)) => {
                self.mil = Some(mil);
                self.message = format!(
                    "Codes read · MIL {} · {} stored, {} pending, {} permanent",
                    if mil.mil_on { "on" } else { "off" },
                    stored.len(),
                    pending.len(),
                    permanent.len()
                );
                self.stored = stored;
                self.pending = pending;
                self.permanent = permanent;
                self.tone = Tone::Good;
            }
            Err(error) => self.fail("Code read failed", error),
        }
        cx.notify();
    }

    fn clear_codes(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let result: Result<(MilStatus, Vec<String>, Vec<String>, Vec<String>)> = (|| {
            let transport = self
                .transport
                .as_mut()
                .ok_or_else(|| CoreError::Transport("not connected".into()))?;
            let mut session = ObdSession::with_default_timeout(transport);
            session.clear_dtcs()?;
            let mil = session.read_mil_status()?;
            let stored = codes(session.read_dtcs(DtcMode::Stored)?);
            let pending = codes(session.read_dtcs(DtcMode::Pending)?);
            let permanent = codes(session.read_dtcs(DtcMode::Permanent)?);
            Ok((mil, stored, pending, permanent))
        })();

        match result {
            Ok((mil, stored, pending, permanent)) => {
                self.mil = Some(mil);
                self.stored = stored;
                self.pending = pending;
                self.permanent = permanent;
                self.message = "Stored codes cleared; readiness monitors were reset".into();
                self.tone = Tone::Good;
            }
            Err(error) => self.fail("Clear failed", error),
        }
        cx.notify();
    }

    fn select_pack(&mut self, id: String, cx: &mut Context<Self>) {
        self.selected_pack = Some(id);
        self.fault_results.clear();
        cx.notify();
    }

    fn search_faults(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let query = self.value(&self.fault_query, cx);
        let result = match self.diagnostics.as_ref() {
            Some(pack) => pack.search_faults(&query, "en", 100),
            None => Err(CoreError::Unsupported(
                "diagnostic databases are not loaded in this source-only build".into(),
            )),
        };
        match result {
            Ok(rows) => {
                self.message = format!("{} fault record(s) match \"{}\"", rows.len(), query);
                self.fault_results = rows;
                self.tone = Tone::Good;
            }
            Err(error) => self.fail("Fault search failed", error),
        }
        cx.notify();
    }

    fn select_coding_section(&mut self, section: String, cx: &mut Context<Self>) {
        let result = match self.coding.as_ref() {
            Some(pack) => pack.expert(Some(&section), 200),
            None => Err(CoreError::Unsupported(
                "coding databases are not loaded in this source-only build".into(),
            )),
        };
        match result {
            Ok(entries) => {
                self.message = format!("{} coding entr(ies) in {}", entries.len(), section);
                self.coding_section = Some(section);
                self.coding_entries = entries;
                self.coding_detail = None;
                self.tone = Tone::Neutral;
            }
            Err(error) => self.fail("Coding catalogue failed", error),
        }
        cx.notify();
    }

    fn select_coding_entry(&mut self, id: i64, cx: &mut Context<Self>) {
        let result = match self.coding.as_ref() {
            Some(pack) => pack.expert_entry(id),
            None => Err(CoreError::Unsupported(
                "coding databases are not loaded in this source-only build".into(),
            )),
        };
        match result {
            Ok(Some(detail)) => {
                self.coding_detail = Some(detail);
                self.tone = Tone::Neutral;
            }
            Ok(None) => {
                self.coding_detail = None;
                self.message = "Coding entry not found".into();
                self.tone = Tone::Warning;
            }
            Err(error) => self.fail("Coding entry failed", error),
        }
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // Rendering helpers
    // -----------------------------------------------------------------------

    fn tab_button(
        &self,
        tab: Tab,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let selected = self.tab == tab;
        div()
            .id(("tab", tab.index()))
            .px_3()
            .py_2()
            .rounded_sm()
            .text_sm()
            .text_color(if selected { rgb(TEXT) } else { rgb(MUTED) })
            .bg(if selected {
                rgb(ACCENT_DARK)
            } else {
                rgb(HEADER)
            })
            .hover(|style| style.bg(rgb(ACCENT_DARK)))
            .on_click(cx.listener(move |app, _, _, cx| {
                app.tab = tab;
                cx.notify();
            }))
            .child(label)
    }

    fn adapter_option(
        &self,
        adapter: Adapter,
        label: &'static str,
        detail: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let selected = self.adapter == adapter;
        div()
            .id(("adapter", adapter.index()))
            .flex_1()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_3()
            .rounded_sm()
            .border_1()
            .border_color(if selected { rgb(ACCENT) } else { rgb(BORDER) })
            .bg(if selected {
                rgb(ACCENT_DARK)
            } else {
                rgb(FIELD)
            })
            .hover(|style| style.bg(rgb(ACCENT_DARK)))
            .on_click(cx.listener(move |app, _, _, cx| {
                if app.adapter != adapter {
                    app.clear_session_data();
                    app.message = format!("Adapter set to {}; not connected", adapter.label());
                    app.tone = Tone::Warning;
                }
                app.adapter = adapter;
                cx.notify();
            }))
            .child(div().text_sm().child(label))
            .child(div().text_xs().text_color(rgb(MUTED)).child(detail))
    }

    fn field_row(&self, label: &'static str, field: &Entity<TextField>) -> gpui::Div {
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .w(px(180.0))
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(label),
            )
            .child(div().flex_1().child(field.clone()))
    }

    fn card(&self) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .p_4()
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(BORDER))
            .rounded_lg()
    }

    fn button(
        &self,
        id: &'static str,
        label: &'static str,
        primary: bool,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .px_3()
            .py_2()
            .rounded_sm()
            .text_sm()
            .text_color(rgb(TEXT))
            .bg(if primary { rgb(ACCENT) } else { rgb(0x263746) })
            .border_1()
            .border_color(if primary {
                rgb(ACCENT)
            } else {
                rgb(BORDER_STRONG)
            })
            .hover(|style| style.bg(rgb(0x30495d)))
            .child(label)
    }

    fn status_panel(&self, text: &str, tone: Tone) -> gpui::Div {
        div()
            .w_full()
            .px_3()
            .py_2()
            .rounded_sm()
            .border_1()
            .border_color(status_color(tone))
            .bg(rgb(FIELD))
            .text_size(px(13.0))
            .text_color(status_color(tone))
            .child(text.to_string())
    }

    fn info_row(&self, label: &'static str, value: impl Into<SharedString>) -> gpui::Div {
        div()
            .flex()
            .items_start()
            .gap_3()
            .py_1()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .w(px(190.0))
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(label),
            )
            .child(div().flex_1().text_sm().child(value.into()))
    }

    fn code_chips(&self, codes: &[String], empty: &str) -> gpui::Div {
        if codes.is_empty() {
            return div()
                .text_sm()
                .text_color(rgb(MUTED))
                .child(empty.to_string());
        }
        div().flex().gap_2().children(
            codes
                .iter()
                .map(|code| {
                    div()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(rgb(ACCENT_DARK))
                        .border_1()
                        .border_color(rgb(BORDER_STRONG))
                        .text_sm()
                        .child(code.clone())
                })
                .collect::<Vec<_>>(),
        )
    }

    fn empty_card(&self, text: &str) -> gpui::Div {
        self.card().child(
            div()
                .text_sm()
                .text_color(rgb(MUTED))
                .child(text.to_string()),
        )
    }

    // -----------------------------------------------------------------------
    // Views
    // -----------------------------------------------------------------------

    fn connect_view(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let mut adapters = Vec::new();
        for (adapter, label, detail) in [
            (Adapter::Demo, "Demo vehicle", "No hardware"),
            (Adapter::Elm327, "ELM327 / WiFi", "OBD-II over TCP"),
            (Adapter::Enet, "ENET", "BMW Ethernet"),
            (Adapter::Doip, "DoIP", "ISO 13400"),
        ] {
            adapters.push(self.adapter_option(adapter, label, detail, cx));
        }

        div().p_5().flex().flex_col().gap_4().child(
            self.card()
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::BOLD)
                        .child("Connect a vehicle"),
                )
                .child(div().text_sm().text_color(rgb(MUTED)).child(
                    "Start with the demo vehicle. It answers OBD-II requests without \
                             an adapter; switch to a real transport when you are ready.",
                ))
                .child(div().flex().gap_2().children(adapters))
                .child(self.field_row("Adapter address", &self.address))
                .child(self.field_row("Tester address", &self.source))
                .child(self.field_row("Target address", &self.target))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            self.button("connect", "Connect", true)
                                .on_click(cx.listener(Self::connect)),
                        )
                        .child(
                            self.button("disconnect", "Disconnect", false)
                                .on_click(cx.listener(Self::disconnect)),
                        ),
                )
                .child(self.status_panel(&self.message, self.tone)),
        )
    }

    fn live_view(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let rows = self
            .live
            .iter()
            .map(|row| {
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .w(px(64.0))
                            .text_sm()
                            .text_color(rgb(MUTED))
                            .child(format!("{:02X}", row.pid)),
                    )
                    .child(div().flex_1().text_sm().child(row.name.clone()))
                    .child(
                        div()
                            .w(px(320.0))
                            .text_sm()
                            .text_color(rgb(SUCCESS))
                            .child(row.value.clone()),
                    )
            })
            .collect::<Vec<_>>();

        let table = self.card().child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .pb_2()
                .border_b_1()
                .border_color(rgb(BORDER))
                .child(
                    div()
                        .w(px(64.0))
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("PID"),
                )
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("Parameter"),
                )
                .child(
                    div()
                        .w(px(320.0))
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("Value"),
                ),
        );

        div()
            .p_5()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                self.card()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xl()
                                            .font_weight(FontWeight::BOLD)
                                            .child("Live data"),
                                    )
                                    .child(div().text_sm().text_color(rgb(MUTED)).child(format!(
                                        "{} supported PID(s) · {} decoded value(s)",
                                        self.supported_count,
                                        self.live.len()
                                    ))),
                            )
                            .child(
                                self.button("refresh-live", "Read live data", true)
                                    .on_click(cx.listener(Self::refresh_live)),
                            ),
                    )
                    .child(self.status_panel(&self.message, self.tone)),
            )
            .child(if rows.is_empty() {
                self.empty_card("No live values yet. Connect an adapter and select Read live data.")
            } else {
                table.children(rows)
            })
    }

    fn codes_view(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let mil_text = match self.mil {
            Some(mil) => format!(
                "MIL {} · {} stored code(s)",
                if mil.mil_on { "ON" } else { "off" },
                mil.dtc_count
            ),
            None => "Not read yet".into(),
        };

        div()
            .p_5()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                self.card()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xl()
                                            .font_weight(FontWeight::BOLD)
                                            .child("Diagnostic trouble codes"),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(
                                                if self.mil.map(|mil| mil.mil_on).unwrap_or(false) {
                                                    rgb(WARNING)
                                                } else {
                                                    rgb(MUTED)
                                                },
                                            )
                                            .child(mil_text),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        self.button("read-codes", "Read codes", true)
                                            .on_click(cx.listener(Self::read_codes)),
                                    )
                                    .child(
                                        self.button("clear-codes", "Clear stored codes", false)
                                            .on_click(cx.listener(Self::clear_codes)),
                                    ),
                            ),
                    )
                    .child(self.status_panel(&self.message, self.tone)),
            )
            .child(
                self.card()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Stored"),
                    )
                    .child(self.code_chips(&self.stored, "No stored codes")),
            )
            .child(
                self.card()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Pending"),
                    )
                    .child(self.code_chips(&self.pending, "No pending codes")),
            )
            .child(
                self.card()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Permanent"),
                    )
                    .child(self.code_chips(&self.permanent, "No permanent codes")),
            )
    }

    fn vehicle_view(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .p_5()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                self.card()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xl()
                                            .font_weight(FontWeight::BOLD)
                                            .child("Vehicle"),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(rgb(MUTED))
                                            .child("Identity and emissions information"),
                                    ),
                            )
                            .child(
                                self.button("scan-vehicle", "Scan vehicle", true)
                                    .on_click(cx.listener(Self::scan_vehicle)),
                            ),
                    )
                    .child(self.status_panel(&self.message, self.tone)),
            )
            .child(
                self.card()
                    .child(self.info_row(
                        "VIN",
                        if self.vin.is_empty() {
                            "not reported".to_string()
                        } else {
                            self.vin.clone()
                        },
                    ))
                    .child(self.info_row(
                        "Calibration ID",
                        if self.calibration.is_empty() {
                            "not reported".to_string()
                        } else {
                            self.calibration.clone()
                        },
                    ))
                    .child(self.info_row(
                        "CVN",
                        if self.cvn.is_empty() {
                            "not reported".to_string()
                        } else {
                            self.cvn.clone()
                        },
                    ))
                    .child(self.info_row("Supported PIDs", format!("{}", self.supported_count)))
                    .child(self.info_row("Adapter", self.adapter.label()))
                    .child(
                        self.info_row(
                            "Target address",
                            self.transport
                                .as_ref()
                                .and_then(|transport| transport.target_address())
                                .map(|address| format!("0x{address:04X}"))
                                .unwrap_or_else(|| "not addressed".into()),
                        ),
                    ),
            )
    }

    fn packs_view(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let packs =
            self.catalog
                .packs()
                .iter()
                .enumerate()
                .map(|(index, pack)| {
                    let id = pack.manifest.id.clone();
                    let selected = self.selected_pack.as_deref() == Some(id.as_str());
                    let coverage: Vec<String> = pack
                        .manifest
                        .coverage
                        .iter()
                        .take(4)
                        .map(|(key, value)| format!("{value} {key}"))
                        .collect();
                    div()
                        .id(("pack", index))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_3()
                        .rounded_sm()
                        .border_1()
                        .border_color(if selected { rgb(ACCENT) } else { rgb(BORDER) })
                        .bg(if selected {
                            rgb(ACCENT_DARK)
                        } else {
                            rgb(FIELD)
                        })
                        .hover(|style| style.bg(rgb(ACCENT_DARK)))
                        .on_click(cx.listener(move |app, _, _, cx| app.select_pack(id.clone(), cx)))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(pack.manifest.display_name().to_string()),
                        )
                        .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                            "{} · {}",
                            pack.manifest.scope.as_deref().unwrap_or("data"),
                            pack.manifest.version.as_deref().unwrap_or("unversioned")
                        )))
                        .child(div().text_xs().text_color(rgb(MUTED)).child(
                            if coverage.is_empty() {
                                "no coverage counts".to_string()
                            } else {
                                coverage.join(" · ")
                            },
                        ))
                })
                .collect::<Vec<_>>();

        let detail = self.pack_detail(cx);

        div()
            .p_5()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                self.card()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::BOLD)
                            .child("Vehicle data packs"),
                    )
                    .child(div().text_sm().text_color(rgb(MUTED)).child(format!(
                        "{} pack(s) discovered in {}",
                        self.catalog.packs().len(),
                        self.packs_root.display()
                    )))
                    .child(self.status_panel(&self.message, self.tone))
                    .children(packs),
            )
            .child(detail)
    }

    fn pack_detail(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let Some(id) = self.selected_pack.clone() else {
            return self.empty_card("Select a pack to inspect its contents.");
        };
        match id.as_str() {
            "generic-obd2" => {
                let pids = self
                    .generic
                    .as_ref()
                    .map(|pack| {
                        pack.pids
                            .iter()
                            .take(40)
                            .map(|pid| {
                                div()
                                    .flex()
                                    .gap_3()
                                    .py_1()
                                    .border_b_1()
                                    .border_color(rgb(BORDER))
                                    .child(
                                        div()
                                            .w(px(56.0))
                                            .text_sm()
                                            .text_color(rgb(MUTED))
                                            .child(format!("{:02X}", pid.pid)),
                                    )
                                    .child(div().flex_1().text_sm().child(pid.name.clone()))
                                    .child(
                                        div()
                                            .w(px(120.0))
                                            .text_sm()
                                            .text_color(rgb(MUTED))
                                            .child(pid.unit.clone().unwrap_or_default()),
                                    )
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                self.card()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .child("Generic OBD-II"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(MUTED))
                            .child("Standard Mode 01/03/04/07/09/0A definitions."),
                    )
                    .children(pids)
            }
            "bmw-diagnostics" => {
                let (source, version, ecus) = match self.diagnostics.as_ref() {
                    Some(pack) => {
                        let meta = pack.meta().unwrap_or_default();
                        (
                            meta.get("source").cloned().unwrap_or_default(),
                            meta.get("dataset_version").cloned().unwrap_or_default(),
                            meta.get("counts").cloned().unwrap_or_default(),
                        )
                    }
                    None => (String::new(), String::new(), String::new()),
                };
                let results = self
                    .fault_results
                    .iter()
                    .take(40)
                    .map(|row| {
                        div()
                            .flex()
                            .gap_3()
                            .py_1()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .child(
                                div()
                                    .w(px(90.0))
                                    .text_sm()
                                    .text_color(rgb(ACCENT))
                                    .child(row.code.clone()),
                            )
                            .child(
                                div()
                                    .w(px(56.0))
                                    .text_sm()
                                    .text_color(rgb(MUTED))
                                    .child(row.kind.clone()),
                            )
                            .child(div().flex_1().text_sm().child(row.text.clone()))
                    })
                    .collect::<Vec<_>>();
                self.card()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .child("BMW diagnostics"),
                    )
                    .child(self.info_row("Source", source))
                    .child(self.info_row("Dataset version", version))
                    .child(self.info_row("Coverage", ecus))
                    .child(self.field_row("Fault search", &self.fault_query))
                    .child(
                        self.button("search-faults", "Search", true)
                            .on_click(cx.listener(Self::search_faults)),
                    )
                    .children(results)
            }
            "bmw-coding" => {
                let (version, counts, sections) = match self.coding.as_ref() {
                    Some(pack) => (
                        pack.data_version().ok().flatten().unwrap_or_default(),
                        pack.meta()
                            .ok()
                            .and_then(|meta| meta.get("counts").cloned())
                            .unwrap_or_default(),
                        pack.sections().map(|rows| rows.len()).unwrap_or(0),
                    ),
                    None => (String::new(), String::new(), 0),
                };
                self.card()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .child("BMW coding"),
                    )
                    .child(self.info_row("Data version", version))
                    .child(self.info_row("Coverage", counts))
                    .child(self.info_row("Sections", format!("{sections}")))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(MUTED))
                            .child("Browse expert entries on the Coding tab."),
                    )
            }
            _ => self.empty_card("No detail view for this pack yet."),
        }
    }

    fn coding_view(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        if self.coding.is_none() {
            return div().p_5().child(self.empty_card(
                "This source-only build does not load coding databases. Database reader APIs remain available in the core library for explicit integration with authorised data.",
            ));
        }

        let sections = self
            .coding_sections
            .iter()
            .enumerate()
            .map(|(index, (section, count))| {
                let section_name = section.clone();
                let selected = self.coding_section.as_deref() == Some(section.as_str());
                div()
                    .id(("section", index))
                    .px_3()
                    .py_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(if selected { rgb(ACCENT) } else { rgb(BORDER) })
                    .bg(if selected {
                        rgb(ACCENT_DARK)
                    } else {
                        rgb(FIELD)
                    })
                    .hover(|style| style.bg(rgb(ACCENT_DARK)))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.select_coding_section(section_name.clone(), cx)
                    }))
                    .child(div().text_sm().child(format!("{section} ({count})")))
            })
            .collect::<Vec<_>>();

        let entries = self
            .coding_entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let id = entry.id;
                let selected = self
                    .coding_detail
                    .as_ref()
                    .map(|detail| detail.entry.id == id)
                    .unwrap_or(false);
                div()
                    .id(("entry", index))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .rounded_sm()
                    .border_1()
                    .border_color(if selected { rgb(ACCENT) } else { rgb(BORDER) })
                    .bg(if selected {
                        rgb(ACCENT_DARK)
                    } else {
                        rgb(FIELD)
                    })
                    .hover(|style| style.bg(rgb(ACCENT_DARK)))
                    .on_click(cx.listener(move |app, _, _, cx| app.select_coding_entry(id, cx)))
                    .child(div().text_sm().child(entry.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(entry.file.clone()),
                    )
            })
            .collect::<Vec<_>>();

        let detail = match &self.coding_detail {
            Some(detail) => {
                let mut options = Vec::new();
                for option in &detail.options {
                    let functions = option
                        .functions
                        .iter()
                        .map(|function| {
                            div()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .child(format!("{} {}", function.name, function.values))
                        })
                        .collect::<Vec<_>>();
                    options.push(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(SUCCESS))
                                    .child(option.name.clone()),
                            )
                            .children(functions),
                    );
                }
                self.card()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .child(detail.entry.name.clone()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(MUTED))
                            .child(detail.entry.description.clone().unwrap_or_default()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(format!("CAFD file: {}", detail.entry.file)),
                    )
                    .children(options)
            }
            None => self.empty_card("Select a coding entry to see its options."),
        };

        div()
            .p_5()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                self.card()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::BOLD)
                            .child("Coding catalogue"),
                    )
                    .child(div().text_sm().text_color(rgb(MUTED)).child(format!(
                        "{} section(s) · read-only browser",
                        self.coding_sections.len()
                    )))
                    .child(self.status_panel(&self.message, self.tone))
                    .child(div().flex().flex_wrap().gap_2().children(sections)),
            )
            .child(
                div()
                    .flex()
                    .gap_4()
                    .child(div().flex_1().child(self.card().children(entries)))
                    .child(div().flex_1().child(detail)),
            )
    }
}

impl Render for App {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = div()
            .flex()
            .flex_col()
            .w_full()
            .bg(rgb(HEADER))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_5()
                    .py_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::BOLD)
                                    .child("OBD-II Studio"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .child("Open diagnostics and vehicle data packs"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .w(px(560.0))
                            .text_sm()
                            .text_color(status_color(self.tone))
                            .child(
                                div()
                                    .w(px(8.0))
                                    .h(px(8.0))
                                    .rounded_full()
                                    .bg(status_color(self.tone)),
                            )
                            .child(div().truncate().child(self.message.clone())),
                    ),
            )
            .child(div().flex().gap_1().px_5().pb_3().children([
                self.tab_button(Tab::Connect, "Connect", cx),
                self.tab_button(Tab::Live, "Live", cx),
                self.tab_button(Tab::Codes, "Codes", cx),
                self.tab_button(Tab::Vehicle, "Vehicle", cx),
                self.tab_button(Tab::Packs, "Packs", cx),
                self.tab_button(Tab::Coding, "Coding", cx),
            ]));

        let current_view = match self.tab {
            Tab::Connect => self.connect_view(cx),
            Tab::Live => self.live_view(cx),
            Tab::Codes => self.codes_view(cx),
            Tab::Vehicle => self.vehicle_view(cx),
            Tab::Packs => self.packs_view(cx),
            Tab::Coding => self.coding_view(cx),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(CANVAS))
            .text_color(rgb(TEXT))
            .child(header)
            .child(
                div()
                    .id("main-scroll")
                    .flex_1()
                    .w_full()
                    .overflow_y_scroll()
                    .child(current_view),
            )
    }
}

impl Tab {
    fn index(self) -> usize {
        match self {
            Self::Connect => 0,
            Self::Live => 1,
            Self::Codes => 2,
            Self::Vehicle => 3,
            Self::Packs => 4,
            Self::Coding => 5,
        }
    }
}

impl Adapter {
    fn label(self) -> &'static str {
        match self {
            Self::Demo => "Demo vehicle",
            Self::Elm327 => "ELM327 / WiFi",
            Self::Enet => "ENET",
            Self::Doip => "DoIP",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Demo => 0,
            Self::Elm327 => 1,
            Self::Enet => 2,
            Self::Doip => 3,
        }
    }
}

fn field(cx: &mut Context<App>, value: &str, placeholder: &str) -> Entity<TextField> {
    cx.new(|cx| TextField::new(cx, value, placeholder))
}

fn codes(list: Vec<obd2_core::Dtc>) -> Vec<String> {
    list.into_iter().map(|code| code.code).collect()
}

fn parse_u8(value: &str) -> Result<u8> {
    let digits = value
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X");
    u8::from_str_radix(digits, 16)
        .map_err(|_| CoreError::Invalid(format!("invalid tester/target address: {value}")))
}

fn parse_u16(value: &str) -> Result<u16> {
    let digits = value
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X");
    u16::from_str_radix(digits, 16)
        .map_err(|_| CoreError::Invalid(format!("invalid tester/target address: {value}")))
}

fn status_color(tone: Tone) -> gpui::Rgba {
    match tone {
        Tone::Neutral => rgb(MUTED),
        Tone::Good => rgb(SUCCESS),
        Tone::Warning => rgb(WARNING),
        Tone::Error => rgb(ERROR),
    }
}

fn main() {
    Application::new().run(|cx: &mut GpuiApp| {
        ui::text_field::bind_keys(cx);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1180.0), px(820.0)), cx);
        let result = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| App::new(cx)),
        );
        if let Ok(window) = result {
            let _ = window.update(cx, |_, _, cx| cx.activate(true));
        }
    });
}
