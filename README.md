# petrinet_plc — Rust port (work in progress)

A Rust + WebAssembly port of **petrinet_plc**, a Petri-net–based soft PLC with a
web IHM. The original Python/JS project lives at:

> https://github.com/henrique-santos-silva/petrinet_plc

## Status — read this first

This is an **unfinished port** and should be treated as experimental:

- **Work in progress.** Not feature-complete with the original (see
  *What is / isn't ported* below).
- **Not manually tested.** The automated tests pass, but the application has
  **not** been exercised by a human through the browser UI in any meaningful
  way.
- **Not battle-tested.** It has never run against real hardware, real plant IO,
  or any production/field workload.
- **100% vibecoded.** It was generated end-to-end by an AI assistant.
- **Not reviewed yet.** No human has reviewed the code for correctness, safety,
  or security.

Do **not** use this to control anything real. Treat every claim here as
"plausible but unverified".

## What it is

A faithful-as-possible reimplementation of the original engine and UI:

- **Engine**: places, arcs (incl. inhibitor), instantaneous & timed
  transitions, priority/rate-weighted firing selection, deadlock & cycle
  detection.
- **Boolean expression parser** for transition signal-enabling and output
  expressions.
- **Web server** that drives the state machine and streams live IO / state /
  Petri-net debug info to the browser.
- **Frontend** (Leptos/WASM): the IHM control panel, the IO emulator, and the
  live Petri-net debug canvas.

### Transport change vs. the original

The original used **socket.io**. This port uses a **native WebSocket** carrying
typed JSON messages defined once in the shared `protocol` crate (used by both
the backend and the WASM frontend), so the client/server contract can't drift.

## What is / isn't ported

| Original (Python/JS) | Status here |
| --- | --- |
| `boolParser.py` | ported (`backend/src/bool_parser.rs`) |
| `petri_net_subcomponents.py` | ported (`backend/src/petri_net_subcomponents.rs`) |
| `petri_net_handler.py` | ported (`backend/src/petri_net_handler.rs`) |
| `io_handlers.py` -> `IOWebMocker` | ported (`backend/src/io_handler.rs`) |
| `state_machine.py` | ported (`backend/src/state_machine.rs`) |
| `webserver_handler.py` (Flask + socket.io) | ported to axum + native WS |
| `run.py` | ported (`backend/src/run.rs`) |
| IHM / IO-mocker / debug-view JS | ported to Leptos/WASM (`frontend/`) |
| XML(PNML)->IOPT conversion + validation (JS) | ported (`frontend/src/iopt.rs`) |
| `io_handlers.py` -> **`PDR0004_IOHandler`** (physical I2C/smbus) | NOT ported |
| `io_handlers.py` -> **`IOHandlersWrapper`** (physical/emulator switch) | NOT ported |
| PyInstaller packaging | not applicable |

Notable simplifications / deviations:

- **No physical IO.** The physical/emulator toggle in the UI and state machine
  is effectively a no-op: physical IO is always reported as unavailable. See the
  `TODO(port)` notes in `backend/src/io_handler.rs` and
  `backend/src/state_machine.rs`.
- **Single server, multiple routes.** The original ran the IO emulator on a
  separate port; here it's just the `/iomocker` route on the same server.
- **Tests are representative, not 1:1** translations of the Python `tst/` suite.

## Architecture (Cargo workspace)

```
protocol/   shared WebSocket message types (backend + frontend)
backend/    PLC engine + axum/WebSocket server (native binary)
frontend/   Leptos CSR app compiled to WASM (built with trunk)
bench/      Python engine benchmark (for comparison vs the original)
```

Routes served by the backend:

- `/`          — IHM control panel
- `/iomocker`  — IO emulator (clickable inputs)
- `/debug`     — live Petri-net canvas
- `/ws`        — WebSocket endpoint
- `/api/getFile/IOPT.json` — download the currently loaded net

> Note: the server binds `0.0.0.0:50000` with **no authentication** (as the
> original did). Anyone who can reach the port can control the PLC and toggle
> IO. Only run it on a trusted/local network.

## Build & run

Prerequisites: a recent Rust toolchain, the `wasm32-unknown-unknown` target, and
[`trunk`](https://trunkrs.dev/).

```bash
# 1. Build the WASM frontend
cd frontend
trunk build --release        # output goes to frontend/dist

# 2. Run the backend (serves frontend/dist)
cd ..
cargo run --release --bin petrinet_plc
# then open http://localhost:50000
```

The backend looks for the frontend bundle at `../frontend/dist` by default;
override with the `PETRINET_STATIC_DIR` environment variable.

Example Petri-net XML files to upload live in the original repo under
`Demo Petri Nets/xml/`.

## Tests

```bash
cargo test                 # backend unit tests + WebSocket integration tests
cd frontend && cargo clippy --target=wasm32-unknown-unknown
```

## Benchmarks

A throughput micro-benchmark of the engine hot path (transition selection +
firing), with a matching Python harness using the **original** code, so the two
are directly comparable:

```bash
# Rust (release)
cargo run --release --example engine_bench -- 2000000 plain
cargo run --release --example engine_bench -- 2000000 signal
cargo run --release --example engine_bench -- 1000000 wide 10000

# Python (original engine)
PYTHONPATH=/path/to/petrinet_plc python3 bench/py_engine_bench.py 1000000
```

## License

Licensed under the **GNU General Public License v3.0** (see `LICENSE`), the same
license as the upstream project. This repository is a derivative work (a Rust
port) of `petrinet_plc` by Henrique Santos Silva:
https://github.com/henrique-santos-silva/petrinet_plc
