# elephant.import-pen

pen.dev importer plugin for [Elephant](https://github.com/rluders/elephant)
(ADR-058). Converts `.pen` documents (schema 2.14–2.25) into `.ele` design and
library files, assets and a loss report. Built to WebAssembly, signed, and
installed on demand; it is not bundled with the app.

- `src/abi.rs` — the guest ABI (`elephant_alloc`, `elephant_import`).
- `src/lib.rs` — `import` / `import_json`, the same code natively and in WASM.
- IDs are derived from the source, so a re-import reuses the ID map.

## Develop

```sh
cargo test
cargo build --target wasm32-unknown-unknown --release
```

The Elephant crates (`domain`, `document`, `plugin-api`) are pinned by git
revision in `Cargo.toml`; bump the `rev` when the plugin API changes.
`ELEPHANT_DESIGN_PEN=<path to design.pen>` enables the test that imports the
Elephant UI file.

## Use in the app

Build the module, then run the app's WASM sandbox test with
`ELEPHANT_IMPORT_PEN_WASM=<path to elephant_import_pen.wasm>`
(`cargo test -p elephant-plugins --test import_pen_wasm`), or package it and
`elephant plugin install <dir>`.

Icon data under `data/` is derived from Lucide (see `data/LUCIDE_LICENSE`).
