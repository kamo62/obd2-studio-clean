# OBD-II Studio

An open-source OBD-II desktop app being built for Windows and macOS, with the goal
of working across all vehicle brands and as many ENET, Bluetooth and Wi-Fi OBD-II
adapters as possible.

We want one app for connecting an adapter, checking vehicle information, reading
fault codes and viewing live data, without needing a different application for
each brand. Bluetooth Classic and Bluetooth Low Energy are both part of that goal.

Version: `0.0.1`. This is an early source-only project, not a finished diagnostic
product. Planned support is not a claim of verified compatibility.

## Compatibility goal

- Standard OBD-II diagnostics across OBD-II-compliant vehicles. Available data
  depends on the model year, market, engine and ECUs.
- ELM327-compatible Wi-Fi/TCP and Bluetooth Classic serial adapters, plus
  explicitly supported Bluetooth Low Energy profiles.
- ENET/HSFZ and DoIP on vehicles that actually use those protocols.
- Manufacturer-specific read features using information we have permission to
  use and distribute, without bundling recovered manufacturer databases.

"All brands and dongles" is the direction, not a guarantee. Some adapters use
proprietary protocols, incomplete ELM implementations or different BLE services.
An adapter name or advertised version is not proof of compatibility. Generic
emissions diagnostics do not imply access to ABS, airbags, body modules, EV
batteries, coding or programming on every vehicle.

## Current capabilities

| Area | What is in the source |
|---|---|
| Desktop | Rust/GPUI views for connection, live values, codes, vehicle identity and packs. Native Windows/macOS release support remains unverified. |
| Demo | An embedded offline vehicle fixture. No adapter opens at startup. |
| Generic diagnostics | Mode 01 PID reads, Mode 03/07/0A code reads, Mode 04 clearing and Mode 09 identity reads. Parsing and coverage are incomplete. |
| Generic definitions | One JSON pack containing 58 PID definitions. |
| Wi-Fi | Explicit TCP connection to an ELM327-compatible endpoint. No automatic Wi-Fi setup or discovery. |
| ENET | HSFZ framing and an explicit-address TCP client. Real-vehicle compatibility is unverified. |
| DoIP | TCP client, routing activation and diagnostic framing. Real-vehicle compatibility is unverified. |
| Bluetooth | No desktop Classic or BLE backend yet. A generic stream abstraction is not Bluetooth support. |
| Manufacturer data | Read-only reader source remains in the core. No manufacturer database is included or automatically opened by the desktop. |
| Coding | Core backup/restore APIs exist. Desktop coding writes are not exposed or supported. |
| Distribution | No verified installer, signed release or hardware compatibility list. |

The inherited checks still expect removed BMW packs and the previous demo
identifier. They also contain an optional local manufacturer-database check.
This snapshot is not presented as having a passing test suite. Do not run those
checks against a folder containing private vehicle data.

## Development

The workspace declares Rust `1.85` and edition `2024`. The minimum version has not
been verified against the locked dependencies. Native toolchain prerequisites and
supported Windows/macOS versions have not yet been established.

With the required toolchain and locked dependencies already available locally,
run from the repository root:

```sh
cargo run --offline --locked -p obd2-desktop
```

Select Connect with Demo vehicle selected, then inspect Live, Codes and Vehicle.
The generic pack is currently loaded from `./packs`, so launching from another
working directory is not supported yet. Offline Cargo commands require the
matching dependency cache; they do not download missing packages.

## Safety

Do not use the application while driving. Real-vehicle work requires the owner's
permission and suitable workshop or bench precautions. Clearing codes changes
vehicle state and can reset emissions readiness. It is not a repair, and the
current desktop clear action does not yet have a separate confirmation step.

Core restore APIs are not a supported vehicle-programming product. No security
unlock algorithms or desktop coding-write workflow are provided.

## Source and data boundaries

- No recovered manufacturer databases, compressed copies, derived vendor pack
  metadata, credentials, vehicle captures or collected reports are included.
- The desktop discovers only `./packs`. It ignores `OBD2_PACKS_DIR` and does not
  search parent folders or automatically open manufacturer databases.
- Discovery rejects symlinked roots, manifests and databases, skips symlinked
  child directories and confines database references to bare local filenames.
  These checks are not an operating-system sandbox.
- Keep runtime backups, captures, logs, exports and private vehicle data out of Git.

## Code layout

- `apps/desktop/`: GPUI desktop application and controls.
- `crates/obd2-core/src/transport/`: ELM327, ENET, DoIP, demo and replay transports.
- `crates/obd2-core/src/obd/`: sessions, PID/formula handling and DTC decoding.
- `crates/obd2-core/src/pack.rs`: generic definitions, discovery and database readers.
- `crates/obd2-core/src/coding/`: coding-block backup/restore APIs.
- `packs/generic-obd2/pack.json`: included generic definitions.
- `fixtures/demo-vehicle.json`: embedded demo fixture.

The desktop depends on the core. OBD decoding does not depend on the UI.

## Licence

Application code: [MIT](LICENSE), retaining the original copyright notice.
The generic pack declares `CC0-1.0`. Neither grants rights to excluded
manufacturer datasets or standards publications.
