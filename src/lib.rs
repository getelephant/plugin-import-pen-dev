//! `elephant.import-pen` (ADR-058): converts a pen.dev document (`.pen`,
//! schema 2.14 – 2.25) into Elephant `.ele` documents, assets, and a list
//! of losses. A pure function: it never touches files; the host previews
//! the result and writes it in one undoable step.
//!
//! Built to WebAssembly as the plugin (`src/abi.rs`), and as an rlib for
//! native tests and development builds of the CLI.

#[cfg(target_arch = "wasm32")]
mod abi;
mod convert;
mod icons;
mod ids;
mod source;
mod values;

use elephant_plugin_api::{ImportFailure, ImportRequest};

pub use convert::import;

/// Oldest and newest `.pen` schema this importer reads (major, minor).
pub const SCHEMA_MIN: (u32, u32) = (2, 14);
pub const SCHEMA_MAX: (u32, u32) = (2, 25);

/// The JSON entry point: an [`ImportRequest`] in, an [`ImportResult`] or
/// an [`ImportFailure`] out.
#[must_use]
pub fn import_json(input: &str) -> String {
    let out = serde_json::from_str::<ImportRequest>(input)
        .map_err(|e| format!("invalid import request: {e}"))
        .and_then(|req| import(&req));
    match out {
        Ok(r) => serde_json::to_string(&r),
        Err(error) => serde_json::to_string(&ImportFailure { error }),
    }
    .unwrap_or_else(|e| format!("{{\"error\":\"cannot encode the result: {e}\"}}"))
}
