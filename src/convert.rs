//! pen.dev → Elephant conversion (ADR-058 mapping table).
//!
//! Order of work: variables become tokens; the document is cut into
//! canvases (one Elephant page each); every node is converted with the
//! theme mode in effect; the documents are laid out with an estimated text
//! measure (the host lays them out again with real fonts); finally frames
//! named `pages/…` are lifted to the top of their canvas and promoted to
//! pages.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use elephant_document::component::{COMPONENT, COMPONENT_SET, INSTANCE, TEXT};
use elephant_document::effects::{DropShadow, Effect, ShadowType};
use elephant_document::layout::{AutoLayout, CrossAlign, Direction, MainAlign, SizeMode, Sizing};
use elephant_document::prototype::PromotedPage;
use elephant_document::tokens::{Mode, Num, Token, TokenKind, TokenRef, TokenSet, TokenValue};
use elephant_document::{
    Bounds, ComponentDef, ComponentRef, ComponentSetDef, EstimateText, FollowSize, ImageFit,
    ImageRef, InstanceData, LibraryLink, ModeChoice, Node, Override, Page, PenFile, Stroke,
    StrokeSides, TextAlign, TextStyle, VariantAxis, VariantValue, settle_layout,
};
use elephant_domain::{FileId, FileKind, NodeId};
use elephant_plugin_api::{
    AssetContent, DefinitionSize, DocumentRole, ImportLoss, ImportRequest, ImportResult,
    ImportedAsset, ImportedDocument, LossLevel,
};
use serde_json::{Map, Value};

use crate::icons::Lucide;
use crate::ids::Ids;
use crate::source::{self, Index, PNode, Source};
use crate::values::{self, Size, Vars};

/// Converts one `.pen` document.
pub fn import(req: &ImportRequest) -> Result<ImportResult, String> {
    let src = source::parse(&req.source)?;
    let mut cx = Cx::new(req, &src);
    cx.run()
}

/// Properties every node type may carry that need no conversion.
const IGNORED: &[&str] = &["context", "metadata", "locked", "placeholder"];

/// Where a converted node sits, for losses and theme resolution.
#[derive(Clone)]
struct At<'p> {
    path: String,
    mode: String,
    parent: Option<NodeId>,
    /// The parent has Auto Layout.
    flow: bool,
    /// The set this node is a variant of (`variants` ID).
    set: Option<&'p str>,
}

struct Cx<'a> {
    req: &'a ImportRequest,
    src: &'a Source,
    index: Index<'a>,
    ids: Ids,
    vars: Vars,
    losses: Vec<ImportLoss>,
    assets: BTreeMap<String, ImportedAsset>,
    counts: BTreeMap<String, u64>,
    follow: Vec<FollowSize>,
    lucide: Option<Result<Lucide, String>>,
    /// Token IDs by variable name (colors and numbers only).
    tokens: HashMap<String, (elephant_domain::TokenId, TokenKind)>,
    modes: HashMap<String, elephant_domain::ModeId>,
    /// The file that holds tokens and the one being converted.
    token_file: FileId,
    current: FileId,
    /// File of each definition (`reusable` node ID).
    def_file: HashMap<String, FileId>,
    /// Detached copies made so far (to number their IDs).
    detached: u64,
}

fn loss(level: LossLevel, code: &str, path: &str, id: Option<&str>, msg: String) -> ImportLoss {
    ImportLoss {
        level,
        code: code.to_owned(),
        path: path.to_owned(),
        source_id: id.map(str::to_owned),
        message: msg,
    }
}

fn join(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_owned()
    } else {
        format!("{path} › {name}")
    }
}

fn zero() -> Bounds {
    Bounds {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
    }
}

/// `WelcomePage — Dark` → `welcome-page-dark`.
fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if c.is_ascii_uppercase() && prev_lower && !out.ends_with('-') {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
            prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        } else {
            if !out.is_empty() && !out.ends_with('-') {
                out.push('-');
            }
            prev_lower = false;
        }
    }
    out.trim_matches('-').to_owned()
}

/// Whether a frame only groups other frames (a board of boards).
fn is_grouping(n: &PNode) -> bool {
    n.ty == "frame"
        && n.children.len() >= 2
        && n.children
            .iter()
            .all(|c| matches!(c.ty.as_str(), "frame" | "variants" | "group"))
        && !n.is_reusable()
}

/// `Frame 1`, `Group 3`: names pen.dev gives by default.
fn is_generic_name(name: &str) -> bool {
    let mut parts = name.rsplitn(2, ' ');
    let num = parts.next().unwrap_or("");
    let word = parts.next().unwrap_or("");
    matches!(word, "Frame" | "Group") && num.chars().all(|c| c.is_ascii_digit()) && !num.is_empty()
}

fn has_definitions(n: &PNode) -> bool {
    let mut found = false;
    n.walk(&mut |x| found |= x.is_reusable() || x.ty == "variants");
    found
}

fn has_screens(n: &PNode) -> bool {
    let mut found = false;
    n.walk(&mut |x| found |= x.name.starts_with("pages/"));
    found
}

impl<'a> Cx<'a> {
    fn new(req: &'a ImportRequest, src: &'a Source) -> Self {
        let salt = src
            .file_token
            .clone()
            .unwrap_or_else(|| req.source_name.clone());
        let mut ids = Ids::new(&salt, req.id_map.clone());
        let design = ids.file("design");
        Self {
            req,
            src,
            index: Index::new(&src.children),
            ids,
            vars: Vars::new(src),
            losses: Vec::new(),
            assets: BTreeMap::new(),
            counts: BTreeMap::new(),
            follow: Vec::new(),
            lucide: None,
            tokens: HashMap::new(),
            modes: HashMap::new(),
            token_file: design,
            current: design,
            def_file: HashMap::new(),
            detached: 0,
        }
    }

    fn count(&mut self, key: &str) {
        *self.counts.entry(key.to_owned()).or_default() += 1;
    }

    fn lose(&mut self, level: LossLevel, code: &str, at: &str, n: Option<&PNode>, msg: String) {
        self.losses
            .push(loss(level, code, at, n.map(|n| n.id.as_str()), msg));
    }

    // ------------------------------------------------------------- tokens

    fn token_set(&mut self) -> TokenSet {
        let mut set = TokenSet::default();
        if self.src.variables.is_empty() {
            return set;
        }
        for axis in self.vars.other_axes.clone() {
            self.lose(
                LossLevel::Approximated,
                "theme-axis",
                "",
                None,
                format!("theme axis `{axis}` has no Elephant equivalent; only the first axis becomes token modes"),
            );
        }
        let axis = self.vars.axis.clone().unwrap_or_else(|| "mode".into());
        for m in self.vars.modes.clone() {
            let id = self.ids.mode(&format!("{axis}/{m}"));
            self.modes.insert(m.clone(), id);
            set.modes.push(Mode {
                mode_id: id,
                name: m,
                extra: Default::default(),
            });
        }
        for v in self.src.variables.clone() {
            let kind = match v.kind.as_str() {
                "color" => TokenKind::Color,
                "number" if v.name.contains("radius") => TokenKind::Radius,
                "number" => TokenKind::Spacing,
                other => {
                    self.lose(
                        LossLevel::Info,
                        "variable-inlined",
                        &format!("variables › {}", v.name),
                        None,
                        format!("{other} variable `{}` has no token kind; its value is written where it is used", v.name),
                    );
                    continue;
                }
            };
            let id = self.ids.token(&v.name);
            let mut values = BTreeMap::new();
            for m in self.vars.modes.clone() {
                let (resolved, _) = match self.vars.get(&v.name) {
                    Some(_) => {
                        let raw = Value::String(format!("${}", v.name));
                        let (r, n) = self.vars.resolve(&raw, &m);
                        (r.cloned(), n)
                    }
                    None => (None, None),
                };
                let value = match (&kind, resolved) {
                    (TokenKind::Color, Some(Value::String(s))) => {
                        values::color(&s).map(TokenValue::Color)
                    }
                    (_, Some(Value::Number(n))) => n.as_f64().map(|x| TokenValue::Number(Num(x))),
                    _ => None,
                };
                match value {
                    Some(val) => {
                        values.insert(self.modes[&m], val);
                    }
                    None => self.lose(
                        LossLevel::Dropped,
                        "variable-value",
                        &format!("variables › {}", v.name),
                        None,
                        format!("variable `{}` has no usable value in mode `{m}`", v.name),
                    ),
                }
            }
            self.tokens.insert(v.name.clone(), (id, kind.clone()));
            set.tokens.push(Token {
                token_id: id,
                name: v.name.clone(),
                kind,
                values,
                extra: Default::default(),
            });
            self.count("tokens");
        }
        set.canonicalize();
        set
    }

    fn token_ref(&self, var: &str) -> Option<TokenRef> {
        let (id, _) = self.tokens.get(var)?;
        Some(TokenRef {
            token_id: *id,
            file_id: (self.token_file != self.current).then_some(self.token_file),
        })
    }

    // ------------------------------------------------------------- values

    /// A color value (`#…` or `$var`) in `mode`, and the variable it came from.
    fn color_of(&mut self, v: &Value, mode: &str) -> Option<(String, Option<String>)> {
        let (r, var) = self.vars.resolve(v, mode);
        let c = values::color(r?.as_str()?)?;
        Some((c, var))
    }

    /// A number value (`12` or `$var`) in `mode`, and its variable.
    fn number_of(&self, v: &Value, mode: &str) -> Option<(f64, Option<String>)> {
        let (r, var) = self.vars.resolve(v, mode);
        Some((r?.as_f64()?, var))
    }

    fn string_of(&self, v: &Value, mode: &str) -> Option<String> {
        let (r, _) = self.vars.resolve(v, mode);
        r?.as_str().map(str::to_owned)
    }

    fn bind(&self, node: &mut Node, field: &str, var: Option<String>) {
        if let Some(r) = var.and_then(|v| self.token_ref(&v)) {
            node.tokens.insert(field.to_owned(), r);
        }
    }

    // ------------------------------------------------------------- assets

    fn lucide(&mut self) -> Option<&Lucide> {
        if self.lucide.is_none() {
            self.lucide = Some(Lucide::load());
        }
        match self.lucide.as_ref() {
            Some(Ok(l)) => Some(l),
            _ => None,
        }
    }

    fn icon_asset(
        &mut self,
        library: &str,
        name: &str,
        color: &str,
    ) -> Option<elephant_domain::AssetId> {
        let key = format!("icon/{library}/{name}/{color}");
        if let Some(a) = self.assets.get(&key) {
            return a.asset_id.parse().ok();
        }
        if !library.eq_ignore_ascii_case("lucide") {
            return None;
        }
        let svg = self.lucide()?.svg(name, color)?;
        let id = self.ids.asset(&key);
        self.assets.insert(
            key,
            ImportedAsset {
                asset_id: id.to_string(),
                name: format!("{name}.svg"),
                content: AssetContent::Svg(svg),
            },
        );
        self.count("icons");
        Some(id)
    }

    fn file_asset(&mut self, url: &str) -> elephant_domain::AssetId {
        let key = format!("file/{url}");
        if let Some(a) = self.assets.get(&key)
            && let Ok(id) = a.asset_id.parse()
        {
            return id;
        }
        let id = self.ids.asset(&key);
        let name = url.rsplit('/').next().unwrap_or(url).to_owned();
        self.assets.insert(
            key,
            ImportedAsset {
                asset_id: id.to_string(),
                name,
                content: AssetContent::File(url.to_owned()),
            },
        );
        self.count("images");
        id
    }

    // ------------------------------------------------------------- canvases

    /// The canvases of the document: (name, key, node).
    fn canvases(&self) -> Vec<(String, &'a PNode)> {
        let mut out = Vec::new();
        let mut loose = Vec::new();
        for t in &self.src.children {
            if is_grouping(t) {
                for c in &t.children {
                    if is_grouping(c) && is_generic_name(&c.name) {
                        out.extend(c.children.iter().map(|g| (g.name.clone(), g)));
                    } else {
                        out.push((c.name.clone(), c));
                    }
                }
            } else {
                loose.push(t);
            }
        }
        // Loose top-level nodes each get a canvas too (named after them).
        out.extend(loose.into_iter().map(|t| (t.name.clone(), t)));
        out
    }

    // ------------------------------------------------------------- run

    fn run(&mut self) -> Result<ImportResult, String> {
        for k in &self.src.unknown_keys {
            self.losses.push(loss(
                LossLevel::Info,
                "unknown-key",
                "",
                None,
                format!("top-level key `{k}` is not converted"),
            ));
        }
        let stem = self.req.options.file_stem.clone().unwrap_or_else(|| {
            let n = self
                .req
                .source_name
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("design");
            n.strip_suffix(".pen").unwrap_or(n).to_owned()
        });
        let canvases = self.canvases();
        let split = self.req.options.split_library
            && canvases
                .iter()
                .any(|(_, n)| has_definitions(n) && !has_screens(n));
        let design_id = self.ids.file("design");
        let library_id = self.ids.file("library");
        let lib_of = |n: &PNode| split && has_definitions(n) && !has_screens(n);
        // Definitions → file, before any instance is converted.
        for (_, c) in &canvases {
            let file = if lib_of(c) { library_id } else { design_id };
            c.walk(&mut |x| {
                if x.is_reusable() || x.ty == "variants" {
                    self.def_file.insert(x.id.clone(), file);
                }
            });
        }
        self.token_file = if split { library_id } else { design_id };
        self.current = self.token_file;
        let tokens = self.token_set();

        // Never `PenFile::new`/`Page::new`: they draw random IDs, which the
        // WebAssembly sandbox cannot provide.
        let file = |file_id, kind, name: String| PenFile {
            schema_version: elephant_document::pen_schema().current(),
            file_id,
            name,
            kind,
            libraries: Vec::new(),
            tokens: TokenSet::default(),
            code: elephant_document::CodeLinks::default(),
            data: Default::default(),
            pages: Vec::new(),
            extra: Default::default(),
        };
        let mut design = file(design_id, FileKind::Design, stem.clone());
        let mut library = file(
            library_id,
            FileKind::Library,
            format!("{stem} design system"),
        );
        if split {
            library.tokens = tokens;
            design.libraries.push(LibraryLink::linked(library_id));
        } else {
            design.tokens = tokens;
        }

        for (name, c) in &canvases {
            let in_lib = lib_of(c);
            self.current = if in_lib { library_id } else { design_id };
            let mut page = Page {
                page_id: self.ids.page(&c.id),
                name: name.clone(),
                root_node_ids: Vec::new(),
                annotations: Vec::new(),
                review: Default::default(),
                nodes: Vec::new(),
                extra: Default::default(),
            };
            let mut nodes = Vec::new();
            let at = At {
                path: String::new(),
                mode: self.vars.default_mode().to_owned(),
                parent: None,
                flow: false,
                set: None,
            };
            if let Some(root) = self.convert(c, &at, &mut nodes) {
                if let Some(n) = nodes.iter_mut().find(|n| n.id == root)
                    && let Some(b) = &mut n.bounds
                {
                    b.x = 0.0;
                    b.y = 0.0;
                }
                page.root_node_ids.push(root);
            }
            page.nodes = nodes;
            page.canonicalize();
            self.count("canvases");
            if in_lib {
                library.pages.push(page);
            } else {
                design.pages.push(page);
            }
        }

        let mut files = if split {
            vec![design, library]
        } else {
            vec![design]
        };
        settle_layout(&mut files, &self.follow, &EstimateText);
        if self.req.options.promote_pages {
            for f in &mut files {
                let mut order = 0;
                let mut routes = BTreeSet::new();
                for p in &mut f.pages {
                    self.promote(p, &mut order, &mut routes);
                }
            }
        }

        let mut documents = Vec::new();
        for mut f in files {
            f.canonicalize();
            let role = if f.kind == FileKind::Library {
                DocumentRole::Library
            } else {
                DocumentRole::Design
            };
            let path = if role == DocumentRole::Library {
                let lib = self
                    .req
                    .options
                    .library_stem
                    .clone()
                    .unwrap_or_else(|| format!("{stem}-design-system"));
                format!("{lib}.ele")
            } else {
                format!("{stem}.ele")
            };
            self.counts
                .insert(format!("nodes:{path}"), f.node_count() as u64);
            documents.push(ImportedDocument {
                path,
                role,
                document: serde_json::to_value(&f).map_err(|e| e.to_string())?,
            });
        }
        if self.counts.get("icons").copied().unwrap_or(0) > 0 {
            self.losses.push(loss(
                LossLevel::Info,
                "icon-color",
                "",
                None,
                "icons are SVG assets in the color of the theme mode they were imported in; switching modes does not recolor them".into(),
            ));
        }
        self.losses.sort();
        let definition_sizes = self
            .follow
            .iter()
            .map(|f| DefinitionSize {
                node_id: f.node.to_string(),
                width: f.width,
                height: f.height,
            })
            .collect();
        Ok(ImportResult {
            documents,
            assets: self.assets.values().cloned().collect(),
            losses: std::mem::take(&mut self.losses),
            id_map: self.ids.used.clone(),
            counts: self.counts.clone(),
            definition_sizes,
        })
    }

    // ------------------------------------------------------------- nodes

    /// Converts `n` and its subtree into `out`. Returns the node's ID, or
    /// `None` when it was dropped.
    fn convert(&mut self, n: &PNode, at: &At<'_>, out: &mut Vec<Node>) -> Option<NodeId> {
        let path = join(&at.path, &n.name);
        match n.ty.as_str() {
            "frame" | "group" | "variants" => Some(self.container(n, at, &path, out)),
            "text" => Some(self.text(n, at, &path, out)),
            "rectangle" | "ellipse" | "polygon" | "line" => Some(self.shape(n, at, &path, out)),
            "icon" | "icon_font" => Some(self.icon(n, at, &path, out)),
            "ref" => self.instance(n, at, &path, out),
            "path" => self.path(n, at, &path, out),
            "note" | "script" | "browser" | "shader" | "connection" => {
                self.lose(
                    LossLevel::Dropped,
                    "unsupported-node",
                    &path,
                    Some(n),
                    format!("`{}` nodes are not imported", n.ty),
                );
                None
            }
            other => {
                self.lose(
                    LossLevel::Dropped,
                    "unknown-node",
                    &path,
                    Some(n),
                    format!("unknown node type `{other}`"),
                );
                None
            }
        }
    }

    /// Reports properties of `n` outside `known` (plus the common ones).
    fn check_props(&mut self, n: &PNode, path: &str, known: &[&str]) {
        const COMMON: &[&str] = &[
            "x",
            "y",
            "width",
            "height",
            "enabled",
            "opacity",
            "rotation",
            "flipX",
            "flipY",
            "layoutPosition",
            "theme",
            "effect",
        ];
        let unknown: Vec<String> = n
            .props
            .keys()
            .filter(|k| {
                !COMMON.contains(&k.as_str())
                    && !known.contains(&k.as_str())
                    && !IGNORED.contains(&k.as_str())
            })
            .cloned()
            .collect();
        for k in unknown {
            self.lose(
                LossLevel::Dropped,
                "unknown-property",
                path,
                Some(n),
                format!("property `{k}` of a `{}` is not converted", n.ty),
            );
        }
    }

    /// Bounds, sizing, visibility, opacity, rotation, position, and theme.
    #[allow(clippy::too_many_lines)]
    fn base(
        &mut self,
        n: &PNode,
        node: &mut Node,
        at: &At<'_>,
        path: &str,
        hugs: bool,
        unset: (Size, Size),
    ) -> String {
        node.parent_id = at.parent;
        let x = n.num("x").unwrap_or(0.0);
        let y = n.num("y").unwrap_or(0.0);
        let mut sizing = Sizing::default();
        let dim = |this: &mut Self, s: Size, fallback: Size, axis: &str| -> (f64, SizeMode) {
            let s = if s == Size::Unset { fallback } else { s };
            match s {
                Size::Fixed(v) => (v, SizeMode::Fixed),
                Size::Fill(f) if at.flow => (f.unwrap_or(0.0), SizeMode::Fill),
                Size::Fill(f) => {
                    this.lose(
                        LossLevel::Approximated,
                        "fill-outside-layout",
                        path,
                        Some(n),
                        format!("{axis} fills its container, but the container has no Auto Layout; a fixed size is used"),
                    );
                    (f.unwrap_or(100.0), SizeMode::Fixed)
                }
                Size::Hug(f) if hugs => (f.unwrap_or(0.0), SizeMode::Hug),
                Size::Hug(f) => (f.unwrap_or(0.0), SizeMode::Fixed),
                Size::Unset => (0.0, SizeMode::Fixed),
            }
        };
        let (w, hm) = dim(self, values::size(n.get("width")), unset.0, "width");
        let (h, vm) = dim(self, values::size(n.get("height")), unset.1, "height");
        sizing.horizontal = hm;
        sizing.vertical = vm;
        node.bounds = Some(Bounds {
            x,
            y,
            width: w,
            height: h,
        });
        if !sizing.is_fixed() {
            node.sizing = Some(sizing);
        }
        if n.flag("enabled") == Some(false) {
            node.visible = Some(false);
        }
        if n.num("rotation").is_some_and(|r| r.abs() > 0.01) {
            self.lose(
                LossLevel::Dropped,
                "rotation",
                path,
                Some(n),
                "rotation is not supported; the layer is upright".into(),
            );
        }
        if n.flag("flipX") == Some(true) || n.flag("flipY") == Some(true) {
            self.lose(
                LossLevel::Dropped,
                "flip",
                path,
                Some(n),
                "flipping is not supported".into(),
            );
        }
        if n.str("layoutPosition") == Some("absolute") && at.flow {
            self.lose(
                LossLevel::Approximated,
                "absolute-in-layout",
                path,
                Some(n),
                "absolute position inside Auto Layout is not supported; the layer flows with its siblings".into(),
            );
        }
        // Theme: a mode override for this subtree.
        let mut mode = at.mode.clone();
        if let Some(Value::Object(t)) = n.get("theme") {
            let axis = self.vars.axis.clone().unwrap_or_default();
            for (k, v) in t {
                let Some(v) = v.as_str() else { continue };
                if *k == axis
                    && let Some(id) = self.modes.get(v).copied()
                {
                    mode = v.to_owned();
                    node.modes.push(ModeChoice {
                        file_id: (self.token_file != self.current).then_some(self.token_file),
                        mode_id: id,
                    });
                } else {
                    self.lose(
                        LossLevel::Approximated,
                        "theme",
                        path,
                        Some(n),
                        format!("theme `{k}: {v}` has no token mode"),
                    );
                }
            }
        }
        mode
    }

    /// Fill (color, gradient, or image) of a frame or shape.
    fn fill(&mut self, n: &PNode, node: &mut Node, mode: &str, path: &str, opacity: f64) -> bool {
        let Some(v) = n.get("fill") else { return false };
        let v = match v {
            Value::Array(a) => {
                if a.len() > 1 {
                    self.lose(
                        LossLevel::Approximated,
                        "multiple-fills",
                        path,
                        Some(n),
                        "only the top fill is kept".into(),
                    );
                }
                match a.last() {
                    Some(x) => x.clone(),
                    None => return false,
                }
            }
            other => other.clone(),
        };
        match &v {
            Value::String(_) => {
                if let Some((c, var)) = self.color_of(&v, mode) {
                    node.fill = Some(values::with_opacity(&c, opacity));
                    if opacity >= 1.0 {
                        self.bind(node, "fill", var);
                    }
                    return true;
                }
                self.lose(
                    LossLevel::Dropped,
                    "fill",
                    path,
                    Some(n),
                    format!("fill `{v}` cannot be read"),
                );
                false
            }
            Value::Object(o) => {
                if o.get("enabled").and_then(Value::as_bool) == Some(false) {
                    return false;
                }
                match o.get("type").and_then(Value::as_str) {
                    Some("color") => {
                        let c = o.get("color").cloned().unwrap_or(Value::Null);
                        if let Some((c, var)) = self.color_of(&c, mode) {
                            node.fill = Some(values::with_opacity(&c, opacity));
                            self.bind(node, "fill", var);
                            return true;
                        }
                        false
                    }
                    Some("gradient") => {
                        let stops = o
                            .get("colors")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        let first = stops
                            .first()
                            .and_then(|s| s.get("color").cloned())
                            .and_then(|c| self.color_of(&c, mode));
                        self.lose(
                            LossLevel::Approximated,
                            "gradient",
                            path,
                            Some(n),
                            "gradients are not supported; the first color stop is used".into(),
                        );
                        if let Some((c, _)) = first {
                            node.fill = Some(values::with_opacity(&c, opacity));
                            return true;
                        }
                        false
                    }
                    Some("image") => {
                        let Some(url) = o.get("url").and_then(Value::as_str) else {
                            return false;
                        };
                        let fit = match o.get("mode").and_then(Value::as_str) {
                            Some("contain" | "fit") => ImageFit::Fit,
                            Some("stretch") => ImageFit::Stretch,
                            _ => ImageFit::Fill,
                        };
                        if o.get("transform").is_some() {
                            self.lose(
                                LossLevel::Approximated,
                                "image-transform",
                                path,
                                Some(n),
                                "image crop and offset are not supported; the image covers the layer".into(),
                            );
                        }
                        let id = self.file_asset(url);
                        node.image = Some(ImageRef::new(id, fit));
                        true
                    }
                    other => {
                        self.lose(
                            LossLevel::Dropped,
                            "fill",
                            path,
                            Some(n),
                            format!("fill type `{}` is not supported", other.unwrap_or("?")),
                        );
                        false
                    }
                }
            }
            _ => false,
        }
    }

    fn stroke(&mut self, n: &PNode, node: &mut Node, mode: &str, path: &str) {
        let Some(v) = n.get("stroke").cloned() else {
            return;
        };
        let Some((color, var)) = self.color_of(&v, mode) else {
            self.lose(
                LossLevel::Dropped,
                "stroke",
                path,
                Some(n),
                format!("stroke `{v}` cannot be read"),
            );
            return;
        };
        let mut s = Stroke::solid(color, 1.0);
        match n.get("strokeWidth") {
            Some(Value::Object(o)) => {
                let side = |k: &str| o.get(k).and_then(Value::as_f64).unwrap_or(0.0);
                s.width = 0.0;
                s.sides = Some(StrokeSides {
                    top: side("top"),
                    right: side("right"),
                    bottom: side("bottom"),
                    left: side("left"),
                });
            }
            Some(w) => {
                if let Some((w, _)) = self.number_of(w, mode) {
                    s.width = w;
                }
            }
            None => {}
        }
        if let Some(Value::Array(d)) = n.get("strokeDash") {
            s.dash = d.iter().filter_map(Value::as_f64).collect();
        }
        match n.str("strokeAlignment") {
            Some("center") => s.align = elephant_document::StrokeAlign::Center,
            Some("outside") => s.align = elephant_document::StrokeAlign::Outside,
            _ => {}
        }
        node.stroke = Some(s);
        self.bind(node, "stroke", var);
    }

    fn corner_radius(&mut self, n: &PNode, node: &mut Node, mode: &str, path: &str) {
        let Some(v) = n.get("cornerRadius").cloned() else {
            return;
        };
        match &v {
            Value::Array(a) => {
                let r: Vec<f64> = a
                    .iter()
                    .filter_map(|x| self.number_of(x, mode).map(|(v, _)| v))
                    .collect();
                if r.windows(2).any(|w| (w[0] - w[1]).abs() > 0.01) {
                    self.lose(
                        LossLevel::Approximated,
                        "corner-radius",
                        path,
                        Some(n),
                        "different radii per corner are not supported; the largest is used".into(),
                    );
                }
                node.corner_radius = r.into_iter().reduce(f64::max);
            }
            other => {
                if let Some((r, var)) = self.number_of(other, mode) {
                    node.corner_radius = Some(r);
                    self.bind(node, "cornerRadius", var);
                }
            }
        }
    }

    fn effects(&mut self, n: &PNode, node: &mut Node, mode: &str, path: &str) {
        let list: Vec<Value> = match n.get("effect") {
            Some(Value::Array(a)) => a.clone(),
            Some(v @ Value::Object(_)) => vec![v.clone()],
            _ => return,
        };
        for e in list {
            if e.get("enabled").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            match e.get("type").and_then(Value::as_str) {
                Some("shadow") if e.get("shadowType").and_then(Value::as_str) != Some("inner") => {
                    let c = e.get("color").cloned().unwrap_or(Value::Null);
                    let Some((color, _)) = self.color_of(&c, mode) else {
                        continue;
                    };
                    let off = |k: &str| {
                        e.get("offset")
                            .and_then(|o| o.get(k))
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0)
                    };
                    node.effects.push(Effect::DropShadow(DropShadow {
                        kind: ShadowType::DropShadow,
                        x: off("x"),
                        y: off("y"),
                        blur: e.get("blur").and_then(Value::as_f64).unwrap_or(0.0),
                        spread: e.get("spread").and_then(Value::as_f64).unwrap_or(0.0),
                        color,
                        visible: None,
                    }));
                    self.count("shadows");
                }
                Some(t) => self.lose(
                    LossLevel::Dropped,
                    "effect",
                    path,
                    Some(n),
                    format!(
                        "effect `{}` is not supported",
                        if t == "shadow" { "inner shadow" } else { t }
                    ),
                ),
                None => {}
            }
        }
    }

    fn opacity(&mut self, n: &PNode, path: &str, applied_to: &str) -> f64 {
        let o = n.num("opacity").unwrap_or(1.0);
        if o < 1.0 && !applied_to.is_empty() {
            self.lose(
                LossLevel::Approximated,
                "opacity",
                path,
                Some(n),
                format!("layer opacity is applied to the {applied_to} only"),
            );
        }
        o.clamp(0.0, 1.0)
    }

    /// Auto Layout of a frame (`layout` absent: horizontal).
    fn auto_layout(
        &mut self,
        n: &PNode,
        node: &mut Node,
        mode: &str,
        path: &str,
    ) -> Option<AutoLayout> {
        let dir = match n.str("layout") {
            Some("vertical") => Direction::Vertical,
            Some("horizontal") => Direction::Horizontal,
            Some("none") => return None,
            None if n.ty == "frame" || n.ty == "variants" => Direction::Horizontal,
            None => return None,
            Some(other) => {
                self.lose(
                    LossLevel::Approximated,
                    "layout",
                    path,
                    Some(n),
                    format!("layout `{other}` is not supported; horizontal is used"),
                );
                Direction::Horizontal
            }
        };
        let mut l = AutoLayout {
            direction: dir,
            ..AutoLayout::default()
        };
        if let Some(g) = n.get("gap").cloned()
            && let Some((g, var)) = self.number_of(&g, mode)
        {
            l.gap = g;
            self.bind(node, "gap", var);
        }
        if let Some(p) = n.get("padding").cloned() {
            let vars = &self.vars;
            let mut num = |v: &Value| vars.resolve(v, mode).0.and_then(Value::as_f64);
            l.padding = values::padding(&p, &mut num).unwrap_or_default();
        }
        l.align = match n.str("justifyContent") {
            Some("center") => MainAlign::Center,
            Some("end") => MainAlign::End,
            Some("space_between") => MainAlign::SpaceBetween,
            Some(s @ ("space_around" | "space_evenly")) => {
                self.lose(
                    LossLevel::Approximated,
                    "justify",
                    path,
                    Some(n),
                    format!("`{s}` is approximated by space between"),
                );
                MainAlign::SpaceBetween
            }
            _ => MainAlign::Start,
        };
        l.cross_align = match n.str("alignItems") {
            Some("center") => CrossAlign::Center,
            Some("end") => CrossAlign::End,
            Some("stretch") => {
                self.lose(
                    LossLevel::Approximated,
                    "align-stretch",
                    path,
                    Some(n),
                    "stretching children is not supported; they keep their size".into(),
                );
                CrossAlign::Start
            }
            _ => CrossAlign::Start,
        };
        Some(l)
    }

    /// Frames, groups, definitions, and component sets.
    #[allow(clippy::too_many_lines)]
    fn container(&mut self, n: &PNode, at: &At<'_>, path: &str, out: &mut Vec<Node>) -> NodeId {
        let id = self.ids.node(&n.id);
        let ty = if n.ty == "variants" {
            COMPONENT_SET
        } else if n.is_reusable() {
            COMPONENT
        } else if n.ty == "group" {
            "group"
        } else {
            "frame"
        };
        let mut node = Node::new(id, ty, n.name.clone());
        let mut probe = node.clone();
        let layout = self.auto_layout(n, &mut probe, &at.mode, path);
        let has_layout = layout.is_some();
        // Without a size, a laid-out frame hugs; others take their content's extent.
        let extent = |axis: &str| -> Size {
            let mut max: f64 = 0.0;
            for c in &n.children {
                let pos = c.num(if axis == "w" { "x" } else { "y" }).unwrap_or(0.0);
                let len = match values::size(c.get(if axis == "w" { "width" } else { "height" })) {
                    Size::Fixed(v) => v,
                    _ => 0.0,
                };
                max = max.max(pos + len);
            }
            Size::Fixed(max)
        };
        let unset = if has_layout && ty != COMPONENT_SET {
            (Size::Hug(None), Size::Hug(None))
        } else {
            (extent("w"), extent("h"))
        };
        let mode = self.base(
            n,
            &mut node,
            at,
            path,
            has_layout && ty != COMPONENT_SET,
            unset,
        );
        node.tokens = probe.tokens;
        node.layout = layout;
        let opacity = self.opacity(n, path, if n.children.is_empty() { "" } else { "fill" });
        if !self.fill(n, &mut node, &mode, path, opacity) && ty != "group" && ty != COMPONENT_SET {
            // pen.dev frames without a fill are transparent.
            node.fill = Some("#00000000".into());
        }
        self.stroke(n, &mut node, &mode, path);
        self.corner_radius(n, &mut node, &mode, path);
        self.effects(n, &mut node, &mode, path);
        if n.flag("clip") == Some(true) {
            self.lose(
                LossLevel::Approximated,
                "clip",
                path,
                Some(n),
                "clipping content is not supported; overflowing children are drawn".into(),
            );
        }
        self.check_props(
            n,
            path,
            &[
                "fill",
                "stroke",
                "strokeWidth",
                "strokeDash",
                "strokeAlignment",
                "cornerRadius",
                "layout",
                "gap",
                "padding",
                "justifyContent",
                "alignItems",
                "reusable",
                "clip",
                "slot",
            ],
        );

        // Component data.
        if ty == COMPONENT {
            let mut def = ComponentDef::new(self.ids.component(&n.id));
            if let (Some(set), Some(Value::Object(v))) = (at.set, n.get("reusable")) {
                for (axis, value) in v {
                    let Some(value) = value.as_str() else {
                        continue;
                    };
                    def.variant.insert(
                        self.ids.axis(&format!("{set}/{axis}")),
                        self.ids.value(&format!("{set}/{axis}/{value}")),
                    );
                }
            }
            node.component = Some(def);
            self.count("components");
        }
        if ty == COMPONENT_SET {
            let mut axes = Vec::new();
            if let Some(Value::Object(v)) = n.get("reusable") {
                for (axis, values) in v {
                    let vals = values
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>())
                        .unwrap_or_default();
                    axes.push(VariantAxis {
                        id: self.ids.axis(&format!("{}/{axis}", n.id)),
                        name: axis.clone(),
                        values: vals
                            .into_iter()
                            .map(|x| VariantValue {
                                id: self.ids.value(&format!("{}/{axis}/{x}", n.id)),
                                name: x.to_owned(),
                                extra: Default::default(),
                            })
                            .collect(),
                        extra: Default::default(),
                    });
                }
            }
            node.component_set = Some(ComponentSetDef {
                axes,
                extra: Default::default(),
            });
            self.count("component_sets");
        }

        let child_at = At {
            path: path.to_owned(),
            mode,
            parent: Some(id),
            flow: has_layout && ty != COMPONENT_SET,
            set: (n.ty == "variants").then_some(n.id.as_str()),
        };
        let index = out.len();
        out.push(node);
        let mut children = Vec::new();
        for c in &n.children {
            if let Some(cid) = self.convert(c, &child_at, out) {
                children.push(cid);
            }
        }
        out[index].children = children;
        self.count("frames");
        id
    }

    fn text(&mut self, n: &PNode, at: &At<'_>, path: &str, out: &mut Vec<Node>) -> NodeId {
        let id = self.ids.node(&n.id);
        let mut node = Node::new(id, TEXT, n.name.clone());
        let growth = n.str("textGrowth");
        let unset = match growth {
            Some("fixed-width" | "fixed-width-height") => (Size::Unset, Size::Hug(None)),
            _ => (Size::Hug(None), Size::Hug(None)),
        };
        let mut probe = n.clone();
        if growth.is_none() {
            // Auto size: width and height follow the text.
            probe.props.remove("width");
            probe.props.remove("height");
        } else if growth == Some("fixed-width") {
            probe.props.remove("height");
        }
        let mode = self.base(&probe, &mut node, at, path, true, unset);
        node.text = Some(n.str("content").unwrap_or_default().to_owned());
        let mut style = TextStyle::default();
        if let Some(f) = n.get("fontFamily").and_then(|v| self.string_of(v, &mode)) {
            style.font_family = f;
        }
        if let Some(Value::String(s)) = n.get("fontFamily")
            && s.starts_with('$')
        {
            self.count("font_variables_inlined");
        }
        if let Some((s, _)) = n.get("fontSize").and_then(|v| self.number_of(v, &mode)) {
            style.font_size = s;
        }
        if let Some(w) = n.get("fontWeight").and_then(values::font_weight) {
            style.font_weight = w;
        }
        if let Some(lh) = n.num("lineHeight") {
            // pen.dev line heights are multiples of the font size.
            style.line_height = Some(if lh <= 4.0 { lh * style.font_size } else { lh });
        }
        if let Some(ls) = n.num("letterSpacing") {
            style.letter_spacing = ls;
        }
        style.text_align = match n.str("textAlign") {
            Some("center") => TextAlign::Center,
            Some("right" | "end") => TextAlign::Right,
            Some("justify") => TextAlign::Other("justify".into()),
            _ => TextAlign::Left,
        };
        if matches!(n.str("textAlignVertical"), Some(v) if v != "top") {
            self.lose(
                LossLevel::Approximated,
                "text-vertical-align",
                path,
                Some(n),
                "vertical text alignment is not supported; text sits at the top".into(),
            );
        }
        if n.get("fontStyle").and_then(Value::as_str) == Some("italic") {
            style
                .extra
                .insert("fontStyle".into(), Value::String("italic".into()));
        }
        if style != TextStyle::default() {
            node.text_style = Some(style);
        }
        let opacity = self.opacity(n, path, "");
        if !self.fill(n, &mut node, &mode, path, opacity) {
            node.fill = None;
        }
        self.check_props(
            n,
            path,
            &[
                "content",
                "fill",
                "fontFamily",
                "fontSize",
                "fontWeight",
                "lineHeight",
                "letterSpacing",
                "textAlign",
                "textAlignVertical",
                "textGrowth",
                "fontStyle",
            ],
        );
        out.push(node);
        self.count("texts");
        id
    }

    fn shape(&mut self, n: &PNode, at: &At<'_>, path: &str, out: &mut Vec<Node>) -> NodeId {
        let id = self.ids.node(&n.id);
        let ty = match n.ty.as_str() {
            "ellipse" => "ellipse",
            other => {
                if other != "rectangle" {
                    self.lose(
                        LossLevel::Approximated,
                        "shape",
                        path,
                        Some(n),
                        format!("`{other}` shapes are drawn as rectangles"),
                    );
                }
                "rectangle"
            }
        };
        let mut node = Node::new(id, ty, n.name.clone());
        let mode = self.base(n, &mut node, at, path, false, (Size::Unset, Size::Unset));
        let opacity = self.opacity(n, path, "");
        if !self.fill(n, &mut node, &mode, path, opacity) {
            node.fill = Some("#00000000".into());
        }
        self.stroke(n, &mut node, &mode, path);
        self.corner_radius(n, &mut node, &mode, path);
        self.effects(n, &mut node, &mode, path);
        self.check_props(
            n,
            path,
            &[
                "fill",
                "stroke",
                "strokeWidth",
                "strokeDash",
                "strokeAlignment",
                "cornerRadius",
                "polygonCount",
                "sides",
            ],
        );
        out.push(node);
        self.count("shapes");
        id
    }

    fn icon(&mut self, n: &PNode, at: &At<'_>, path: &str, out: &mut Vec<Node>) -> NodeId {
        let id = self.ids.node(&n.id);
        let mut node = Node::new(id, elephant_document::IMAGE, n.name.clone());
        let mut probe = n.clone();
        for k in ["width", "height"] {
            if values::size(n.get(k)) == Size::Unset {
                probe.props.insert(k.into(), Value::from(24));
            }
        }
        let mode = self.base(
            &probe,
            &mut node,
            at,
            path,
            false,
            (Size::Unset, Size::Unset),
        );
        let opacity = self.opacity(n, path, "");
        let color = n
            .get("fill")
            .cloned()
            .and_then(|v| self.color_of(&v, &mode))
            .map_or_else(|| "#000000".to_owned(), |(c, _)| c);
        let color = values::with_opacity(&color, opacity);
        let library = n
            .str("library")
            .or(n.str("iconFontFamily"))
            .unwrap_or("lucide")
            .to_ascii_lowercase();
        let name = n
            .str("icon")
            .or(n.str("iconFontName"))
            .unwrap_or_default()
            .to_owned();
        match self.icon_asset(&library, &name, &color) {
            Some(asset) => node.image = Some(ImageRef::new(asset, ImageFit::Fit)),
            None => self.lose(
                LossLevel::Dropped,
                "icon",
                path,
                Some(n),
                format!("icon `{name}` of `{library}` is not available"),
            ),
        }
        self.check_props(
            n,
            path,
            &[
                "icon",
                "library",
                "fill",
                "iconFontFamily",
                "iconFontName",
                "weight",
            ],
        );
        out.push(node);
        id
    }

    /// Paths with SVG data become SVG image layers.
    fn path(&mut self, n: &PNode, at: &At<'_>, path: &str, out: &mut Vec<Node>) -> Option<NodeId> {
        let Some(d) = n.str("geometry").or(n.str("d")).map(str::to_owned) else {
            self.lose(
                LossLevel::Dropped,
                "path",
                path,
                Some(n),
                "a path without geometry is not imported".into(),
            );
            return None;
        };
        let id = self.ids.node(&n.id);
        let mut node = Node::new(id, elephant_document::IMAGE, n.name.clone());
        let mode = self.base(n, &mut node, at, path, false, (Size::Unset, Size::Unset));
        let b = node.bounds.unwrap_or_else(zero);
        let fill = n
            .get("fill")
            .cloned()
            .and_then(|v| self.color_of(&v, &mode))
            .map_or_else(|| "none".to_owned(), |(c, _)| c);
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\"><path d=\"{d}\" fill=\"{fill}\"/></svg>\n",
            w = b.width.max(1.0),
            h = b.height.max(1.0),
            d = d.replace('"', "")
        );
        let key = format!("path/{}", n.id);
        let asset = self.ids.asset(&key);
        self.assets.insert(
            key,
            ImportedAsset {
                asset_id: asset.to_string(),
                name: format!("{}.svg", slug(&n.name)),
                content: AssetContent::Svg(svg),
            },
        );
        node.image = Some(ImageRef::new(asset, ImageFit::Stretch));
        self.lose(
            LossLevel::Approximated,
            "path-as-image",
            path,
            Some(n),
            "the path is imported as an SVG image layer".into(),
        );
        out.push(node);
        Some(id)
    }

    // ------------------------------------------------------------- instances

    /// The definition a `ref` (with its `variant` choice) points at.
    fn definition(
        &mut self,
        target: &'a PNode,
        variant: Option<&Map<String, Value>>,
        path: &str,
        n: &PNode,
    ) -> Option<&'a PNode> {
        if target.ty != "variants" {
            return target.is_reusable().then_some(target);
        }
        let axes = target
            .get("reusable")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let want: BTreeMap<&str, String> = axes
            .iter()
            .map(|(a, vals)| {
                let chosen = variant
                    .and_then(|v| v.get(a))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| {
                        vals.as_array()
                            .and_then(|x| x.first())
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_default();
                (a.as_str(), chosen)
            })
            .collect();
        let score = |c: &PNode| -> usize {
            c.get("reusable").and_then(Value::as_object).map_or(0, |r| {
                want.iter()
                    .filter(|(a, v)| r.get(**a).and_then(Value::as_str) == Some(v.as_str()))
                    .count()
            })
        };
        let best = target.children.iter().max_by_key(|c| score(c))?;
        if score(best) < want.len() {
            self.lose(
                LossLevel::Approximated,
                "variant",
                path,
                Some(n),
                format!(
                    "no variant of `{}` matches {want:?}; the closest is used",
                    target.name
                ),
            );
        }
        Some(best)
    }

    fn component_ref(&self, def: &PNode) -> ComponentRef {
        let mut r = ComponentRef::local(ComponentId::from_u128(0));
        let file = self.def_file.get(&def.id).copied().unwrap_or(self.current);
        r.file_id = (file != self.current).then_some(file);
        r
    }

    #[allow(clippy::too_many_lines)]
    fn instance(
        &mut self,
        n: &PNode,
        at: &At<'_>,
        path: &str,
        out: &mut Vec<Node>,
    ) -> Option<NodeId> {
        let Some(target_id) = n.str("ref") else {
            self.lose(
                LossLevel::Dropped,
                "ref",
                path,
                Some(n),
                "an instance without a component".into(),
            );
            return None;
        };
        let Some(target) = self.index.get(target_id) else {
            self.lose(
                LossLevel::Dropped,
                "missing-component",
                path,
                Some(n),
                format!("component `{target_id}` is not in the document"),
            );
            return None;
        };
        let def = self.definition(target, n.get("variant").and_then(Value::as_object), path, n)?;
        let descendants = n
            .get("descendants")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let structural = descendants.values().any(|v| {
            v.as_object()
                .is_some_and(|o| o.contains_key("children") || o.contains_key("type"))
        });
        if structural {
            let copy = self.materialize(n, def, &descendants);
            self.lose(
                LossLevel::Approximated,
                "detached",
                path,
                Some(n),
                "the instance replaces layers of its component, so it is imported detached".into(),
            );
            self.count("detached_instances");
            return self.convert(&copy, at, out);
        }

        let id = self.ids.node(&n.id);
        let mut node = Node::new(id, INSTANCE, n.name.clone());
        let follow_w = values::size(n.get("width")) == Size::Unset;
        let follow_h = values::size(n.get("height")) == Size::Unset;
        let mode = self.base(n, &mut node, at, path, false, (Size::Unset, Size::Unset));
        if follow_w || follow_h {
            self.follow.push(FollowSize {
                node: id,
                width: follow_w,
                height: follow_h,
            });
        }
        let mut cref = self.component_ref(def);
        cref.component_id = self.ids.component(&def.id);
        let mut data = InstanceData::new(cref);
        // Root overrides the engine supports: fill, radius, stroke.
        if n.get("fill").is_some() {
            self.fill(n, &mut node, &mode, path, 1.0);
        }
        if n.get("cornerRadius").is_some() {
            self.corner_radius(n, &mut node, &mode, path);
        }
        if n.get("stroke").is_some() {
            self.stroke(n, &mut node, &mode, path);
        }
        self.effects(n, &mut node, &mode, path);
        for k in ["padding", "gap", "layout", "justifyContent", "alignItems"] {
            if n.get(k).is_some() {
                self.lose(
                    LossLevel::Approximated,
                    "instance-layout",
                    path,
                    Some(n),
                    format!("`{k}` set on an instance is not applied; the component's is used"),
                );
            }
        }
        self.check_props(
            n,
            path,
            &[
                "ref",
                "variant",
                "descendants",
                "fill",
                "cornerRadius",
                "stroke",
                "strokeWidth",
                "strokeDash",
                "padding",
                "gap",
                "layout",
                "justifyContent",
                "alignItems",
            ],
        );
        for (key, props) in &descendants {
            let Some(props) = props.as_object() else {
                continue;
            };
            self.descendant(key, props, &mode, path, n, &mut data);
        }
        data.canonicalize();
        node.instance = Some(data);
        out.push(node);
        self.count("instances");
        Some(id)
    }

    /// One `descendants` entry → an override.
    #[allow(clippy::too_many_lines)]
    fn descendant(
        &mut self,
        key: &str,
        props: &Map<String, Value>,
        mode: &str,
        path: &str,
        n: &PNode,
        data: &mut InstanceData,
    ) {
        let segs: Vec<&str> = key.split('/').collect();
        let target_pen = segs.last().and_then(|s| self.index.get(s));
        let mut ov = Override::new(segs.iter().map(|s| self.ids.node(s)).collect());
        let at = format!("{path} › {}", target_pen.map_or(key, |t| t.name.as_str()));
        let is_icon = target_pen.is_some_and(|t| t.ty == "icon" || t.ty == "icon_font");
        for (k, v) in props {
            match k.as_str() {
                "content" => ov.text = v.as_str().map(str::to_owned),
                "enabled" => ov.visible = v.as_bool(),
                "fill" if !is_icon => match self.color_of(v, mode) {
                    Some((c, _)) => ov.fill = Some(c),
                    None => self.lose(
                        LossLevel::Dropped,
                        "override",
                        &at,
                        Some(n),
                        format!("fill override `{v}` is not a color"),
                    ),
                },
                "fill" | "icon" if is_icon => {}
                "variant" => {
                    let nested = target_pen
                        .filter(|t| t.ty == "ref")
                        .and_then(|t| Some((t, self.index.get(t.str("ref")?)?)));
                    let Some((t, set)) = nested else {
                        self.lose(LossLevel::Dropped, "override", &at, Some(n), "a variant override on a layer that is not an instance".into());
                        continue;
                    };
                    let mut merged = t.get("variant").and_then(Value::as_object).cloned().unwrap_or_default();
                    if let Some(o) = v.as_object() {
                        merged.extend(o.clone());
                    }
                    if let Some(d) = self.definition(set, Some(&merged), &at, n) {
                        let mut r = self.component_ref(d);
                        r.component_id = self.ids.component(&d.id);
                        ov.component = Some(r);
                    }
                }
                other => self.lose(
                    LossLevel::Approximated,
                    "override",
                    &at,
                    Some(n),
                    format!("overriding `{other}` inside an instance is not supported; the component's value is used"),
                ),
            }
        }
        if is_icon && (props.contains_key("icon") || props.contains_key("fill")) {
            let t = target_pen.expect("is_icon");
            let name = props
                .get("icon")
                .and_then(Value::as_str)
                .or(t.str("icon"))
                .unwrap_or_default()
                .to_owned();
            let fill = props
                .get("fill")
                .or(t.get("fill"))
                .cloned()
                .unwrap_or(Value::Null);
            let color = self
                .color_of(&fill, mode)
                .map_or_else(|| "#000000".to_owned(), |(c, _)| c);
            let library = t.str("library").unwrap_or("lucide").to_ascii_lowercase();
            match self.icon_asset(&library, &name, &color) {
                Some(a) => ov.image = Some(ImageRef::new(a, ImageFit::Fit)),
                None => self.lose(
                    LossLevel::Dropped,
                    "icon",
                    &at,
                    Some(n),
                    format!("icon `{name}` is not available"),
                ),
            }
        }
        if !ov.is_empty() {
            data.overrides.push(ov);
        }
    }

    /// A detached copy of `def` for instance `r`: the definition's tree
    /// with the instance's own properties and every override applied.
    fn materialize(&mut self, r: &PNode, def: &PNode, descendants: &Map<String, Value>) -> PNode {
        self.detached += 1;
        let prefix = r.id.clone();
        fn copy(n: &PNode, prefix: &str) -> PNode {
            PNode {
                id: format!("{prefix}/{}", n.id),
                ty: n.ty.clone(),
                name: n.name.clone(),
                props: n.props.clone(),
                children: n.children.iter().map(|c| copy(c, prefix)).collect(),
            }
        }
        let mut root = copy(def, &prefix);
        root.id = r.id.clone();
        root.ty = "frame".into();
        root.name = r.name.clone();
        root.props.remove("reusable");
        for (k, v) in &r.props {
            if !matches!(k.as_str(), "ref" | "descendants" | "variant") {
                root.props.insert(k.clone(), v.clone());
            }
        }
        fn find<'n>(n: &'n mut PNode, id: &str) -> Option<&'n mut PNode> {
            if n.id == id {
                return Some(n);
            }
            n.children.iter_mut().find_map(|c| find(c, id))
        }
        fn replace(n: &mut PNode, id: &str, with: &PNode) -> bool {
            for c in &mut n.children {
                if c.id == id {
                    *c = with.clone();
                    return true;
                }
                if replace(c, id, with) {
                    return true;
                }
            }
            false
        }
        for (key, ov) in descendants {
            let Some(o) = ov.as_object() else { continue };
            let segs: Vec<&str> = key.split('/').collect();
            let first = format!("{prefix}/{}", segs[0]);
            if segs.len() > 1 {
                // Inside a nested instance: pass the override down.
                if let Some(nested) = find(&mut root, &first) {
                    let rest = segs[1..].join("/");
                    let d = nested
                        .props
                        .entry("descendants")
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Value::Object(m) = d {
                        m.insert(rest, ov.clone());
                    }
                }
                continue;
            }
            if o.contains_key("type") {
                if let Ok(with) = PNode::from_value(ov)
                    && !replace(&mut root, &first, &with)
                    && root.id == first
                {
                    root = with;
                }
                continue;
            }
            if let Some(t) = find(&mut root, &first) {
                for (k, v) in o {
                    if k == "children" {
                        t.children = v
                            .as_array()
                            .map(|a| a.iter().filter_map(|x| PNode::from_value(x).ok()).collect())
                            .unwrap_or_default();
                    } else {
                        t.props.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        root
    }

    // ------------------------------------------------------------- pages

    /// Lifts frames named `pages/…` to the top of `page` and promotes them
    /// (ADR-057). The frames that held them are dissolved: their other
    /// children become top-level layers at the same place.
    fn promote(&mut self, page: &mut Page, order: &mut u32, routes: &mut BTreeSet<String>) {
        let screens: Vec<NodeId> = page
            .nodes
            .iter()
            .filter(|n| n.name.starts_with("pages/") && n.node_type == "frame")
            .map(|n| n.id)
            .collect();
        if screens.is_empty() {
            return;
        }
        let mut wrappers = BTreeSet::new();
        for s in &screens {
            let mut cur = page.node(*s).and_then(|n| n.parent_id);
            while let Some(p) = cur {
                if !wrappers.insert(p) {
                    break;
                }
                cur = page.node(p).and_then(|n| n.parent_id);
            }
        }
        // World positions before anything moves.
        let world: HashMap<NodeId, (f64, f64)> = page
            .nodes
            .iter()
            .map(|n| {
                let (mut x, mut y) = (0.0, 0.0);
                let mut cur = Some(n.id);
                let mut guard = 0;
                while let Some(c) = cur {
                    let Some(m) = page.node(c) else { break };
                    if let Some(b) = m.bounds {
                        x += b.x;
                        y += b.y;
                    }
                    cur = m.parent_id;
                    guard += 1;
                    if guard > 512 {
                        break;
                    }
                }
                (n.id, (x, y))
            })
            .collect();
        let mut roots = Vec::new();
        let mut stack: Vec<NodeId> = page.root_node_ids.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            let Some(n) = page.node(id) else { continue };
            if wrappers.contains(&id) {
                stack.extend(n.children.iter().rev().copied());
                continue;
            }
            roots.push(id);
        }
        for id in &wrappers {
            if let Some(n) = page.node(*id)
                && n.fill.as_deref().is_some_and(|f| f != "#00000000")
            {
                let name = n.name.clone();
                self.losses.push(loss(
                    LossLevel::Info,
                    "canvas-frame",
                    &format!("{} › {name}", page.name),
                    None,
                    format!("frame `{name}` only arranged screens; it was dissolved and its background dropped"),
                ));
            }
        }
        page.nodes.retain(|n| !wrappers.contains(&n.id));
        for n in &mut page.nodes {
            if roots.contains(&n.id) {
                n.parent_id = None;
                if let (Some(b), Some((x, y))) = (&mut n.bounds, world.get(&n.id)) {
                    b.x = *x;
                    b.y = *y;
                }
                // Fill sizing has no parent to fill anymore.
                if let Some(s) = &mut n.sizing {
                    if s.horizontal == SizeMode::Fill {
                        s.horizontal = SizeMode::Fixed;
                    }
                    if s.vertical == SizeMode::Fill {
                        s.vertical = SizeMode::Fixed;
                    }
                    if s.is_fixed() {
                        n.sizing = None;
                    }
                }
            }
        }
        page.root_node_ids = roots;
        for s in screens {
            let Some(n) = page.nodes.iter_mut().find(|n| n.id == s) else {
                continue;
            };
            if n.parent_id.is_some() {
                continue;
            }
            let name: String = n
                .name
                .trim_start_matches("pages/")
                .chars()
                .take(100)
                .collect();
            let base = slug(&name);
            let mut route = format!("/{base}");
            let mut i = 2;
            while !routes.insert(route.clone()) {
                route = format!("/{base}-{i}");
                i += 1;
            }
            n.page = Some(PromotedPage {
                name,
                route: Some(route),
                order: *order,
                extra: Default::default(),
            });
            *order += 1;
            self.count("pages");
        }
        page.canonicalize();
    }
}

use elephant_domain::ComponentId;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_names() {
        assert_eq!(slug("WelcomePage — Dark"), "welcome-page-dark");
        assert_eq!(slug("EditorPage — PagesPanel"), "editor-page-pages-panel");
        assert!(is_generic_name("Frame 12"));
        assert!(!is_generic_name("Screens"));
    }
}
