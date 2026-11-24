//! Mapping checks on a small hand-made document covering each node type.

use elephant_document::{Node, PenFile};
use elephant_import_pen::{import, import_json};
use elephant_plugin_api::{AssetContent, ImportOptions, ImportRequest, ImportResult, LossLevel};

const SMALL: &str = include_str!("fixtures/small.pen");

fn run(split: bool) -> ImportResult {
    import(&ImportRequest {
        api_version: 1,
        source_name: "small.pen".into(),
        source: SMALL.into(),
        options: ImportOptions {
            split_library: split,
            ..ImportOptions::default()
        },
        id_map: Default::default(),
    })
    .unwrap()
}

fn files(r: &ImportResult) -> Vec<PenFile> {
    r.documents
        .iter()
        .map(|d| serde_json::from_value(d.document.clone()).unwrap())
        .collect()
}

fn by_name<'f>(f: &'f PenFile, name: &str) -> &'f Node {
    f.pages
        .iter()
        .flat_map(|p| &p.nodes)
        .find(|n| n.name == name)
        .unwrap_or_else(|| panic!("no node {name}"))
}

#[test]
fn converts_tokens_components_instances_and_pages() {
    let r = run(true);
    let fs = files(&r);
    assert_eq!(fs.len(), 2, "design + library");
    let (design, lib) = (&fs[0], &fs[1]);
    assert_eq!(design.name, "small");
    assert_eq!(lib.kind, elephant_domain::FileKind::Library);
    assert_eq!(design.libraries[0].file_id, lib.file_id);

    // Tokens: colors and numbers with light/dark modes; strings inlined.
    let names: Vec<&str> = lib.tokens.modes.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["light", "dark"]);
    assert_eq!(lib.tokens.tokens.len(), 4);
    assert!(r.losses.iter().any(|l| l.code == "variable-inlined"));

    // Component set with axes; variants are components.
    let set = by_name(lib, "ui/Button");
    assert_eq!(set.node_type, "componentSet");
    let axes = &set.component_set.as_ref().unwrap().axes;
    assert_eq!(axes[0].name, "variant");
    assert_eq!(axes[0].values.len(), 2);
    let stroke = set.stroke.as_ref().unwrap();
    assert_eq!(stroke.dash, [4.0, 4.0]);
    assert!(set.tokens.contains_key("stroke"));
    let card = by_name(lib, "Card");
    assert_eq!(card.node_type, "component");
    assert_eq!(
        card.stroke.as_ref().unwrap().side_widths(),
        [0.0, 0.0, 1.0, 0.0]
    );
    assert_eq!(card.effects.len(), 1);
    // Text: string variable resolved, weight parsed, hugging.
    let label = lib.pages[0]
        .nodes
        .iter()
        .find(|n| n.name == "Label" && n.fill.as_deref() == Some("#ffffff"))
        .unwrap();
    let ts = label.text_style.as_ref().unwrap();
    assert_eq!((ts.font_family.as_str(), ts.font_weight), ("Inter", 500.0));
    // Icons are SVG image layers.
    let icon = by_name(lib, "Icon");
    assert_eq!(icon.node_type, "image");
    assert!(icon.image.is_some());

    // Screens are promoted pages at the top of their canvas; the frame
    // that arranged them is dissolved.
    let screens = &design.pages[0];
    let home = by_name(design, "pages/HomePage");
    assert!(home.parent_id.is_none());
    assert_eq!(
        home.page.as_ref().unwrap().route.as_deref(),
        Some("/home-page")
    );
    let dark = by_name(design, "pages/HomePage — Dark");
    assert_eq!(
        dark.page.as_ref().unwrap().route.as_deref(),
        Some("/home-page-dark")
    );
    assert_eq!(dark.modes.len(), 1, "theme becomes a mode choice");
    assert!(screens.nodes.iter().all(|n| n.name != "Screens"));
    assert!(by_name(design, "Header").parent_id.is_none());

    // Instances: variant choice, text and icon overrides, library refs.
    let primary = by_name(design, "Primary");
    let inst = primary.instance.as_ref().unwrap();
    assert_eq!(inst.component.file_id, Some(lib.file_id));
    let btn2 = lib.pages[0]
        .nodes
        .iter()
        .find(|n| n.name == "Button" && n.stroke.is_some())
        .unwrap();
    assert_eq!(
        inst.component.component_id,
        btn2.component.as_ref().unwrap().component_id
    );
    assert!(
        inst.overrides
            .iter()
            .any(|o| o.text.as_deref() == Some("Create"))
    );
    assert!(inst.overrides.iter().any(|o| o.image.is_some()));
    // An instance that replaces a slot's children is detached.
    let detached = by_name(design, "Card");
    assert_eq!(detached.node_type, "frame");
    assert!(by_name(design, "Body text").parent_id.is_some());
    assert!(r.losses.iter().any(|l| l.code == "detached"));
    // In the dark page, token-bound colors take the dark value.
    let dark_btn = design.pages[0]
        .nodes
        .iter()
        .find(|n| n.name == "Button" && n.node_type == "instance")
        .unwrap();
    assert!(dark_btn.instance.is_some());

    // Assets: icons as SVG, image fills as files next to the source.
    assert!(
        r.assets
            .iter()
            .any(|a| matches!(&a.content, AssetContent::Svg(s) if s.contains("<svg")))
    );
    assert!(
        r.assets
            .iter()
            .any(|a| a.content == AssetContent::File("hero.png".into()))
    );
    assert!(r.id_map.keys().any(|k| k == "node:pageA"));
    assert!(
        r.losses
            .iter()
            .all(|l| l.level != LossLevel::Dropped || l.code != "unknown-node")
    );
}

#[test]
fn one_file_without_split_and_stable_ids() {
    let a = run(false);
    let b = run(false);
    assert_eq!(a.documents.len(), 1);
    assert_eq!(a.documents, b.documents);
    let f = &files(&a)[0];
    assert!(f.libraries.is_empty());
    assert_eq!(f.tokens.tokens.len(), 4);
    let inst = by_name(f, "Primary").instance.as_ref().unwrap();
    assert_eq!(inst.component.file_id, None);
}

#[test]
fn json_entry_point_reports_errors() {
    let out =
        import_json(r#"{"apiVersion":1,"sourceName":"a.pen","source":"{\"version\":\"9.0\"}"}"#);
    assert!(out.contains("\"error\"") && out.contains("9.0"), "{out}");
    let ok = import_json(
        &serde_json::json!({
            "apiVersion": 1, "sourceName": "small.pen", "source": SMALL
        })
        .to_string(),
    );
    assert!(ok.contains("\"documents\""));
}
