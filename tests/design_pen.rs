//! Imports the Elephant UI design file when `ELEPHANT_DESIGN_PEN` points at
//! it. The file lives in the app repository, so the test skips without it.

use std::path::PathBuf;

use elephant_import_pen::import;
use elephant_plugin_api::{ImportOptions, ImportRequest, LossLevel};

fn design_pen() -> Option<String> {
    // Point `ELEPHANT_DESIGN_PEN` at the Elephant UI file to run this test.
    let p = PathBuf::from(std::env::var_os("ELEPHANT_DESIGN_PEN")?);
    std::fs::read_to_string(p).ok()
}

#[test]
fn imports_the_elephant_design_file() {
    let Some(source) = design_pen() else {
        eprintln!("ELEPHANT_DESIGN_PEN not set; skipped");
        return;
    };
    let started = std::time::Instant::now();
    let req = ImportRequest {
        api_version: 1,
        source_name: "design.pen".into(),
        source,
        options: ImportOptions {
            split_library: true,
            file_stem: Some("elephant".into()),
            ..ImportOptions::default()
        },
        id_map: Default::default(),
    };
    let r = import(&req).unwrap();
    eprintln!("import took {:?}", started.elapsed());
    eprintln!("counts: {:#?}", r.counts);
    let mut by_code = std::collections::BTreeMap::<(LossLevel, String), usize>::new();
    for l in &r.losses {
        *by_code.entry((l.level, l.code.clone())).or_default() += 1;
    }
    eprintln!("losses: {by_code:#?}");
    assert_eq!(r.documents.len(), 2);
    for d in &r.documents {
        let file: elephant_document::PenFile = serde_json::from_value(d.document.clone()).unwrap();
        let issues = elephant_document::validate(&file);
        let errors: Vec<_> = issues
            .iter()
            .filter(|i| i.severity == elephant_domain::Severity::Error)
            .take(10)
            .collect();
        assert!(errors.is_empty(), "{}: {errors:#?}", d.path);
    }
    // Deterministic: the same input gives the same output.
    let again = import(&req).unwrap();
    assert_eq!(again.documents, r.documents);
}
