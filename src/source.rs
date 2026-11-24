//! The `.pen` document as read: a tree of typed nodes whose properties stay
//! JSON, plus themes and variables. Reading is lenient; conversion decides
//! what each property means and reports what it does not know.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Map, Value};

use crate::{SCHEMA_MAX, SCHEMA_MIN};

/// One source node. `props` holds every key except `id`, `type`, `name`,
/// and `children`.
#[derive(Debug, Clone, PartialEq)]
pub struct PNode {
    pub id: String,
    pub ty: String,
    pub name: String,
    pub props: Map<String, Value>,
    pub children: Vec<PNode>,
}

impl PNode {
    pub fn from_value(v: &Value) -> Result<Self, String> {
        let Value::Object(o) = v else {
            return Err("a node is not an object".into());
        };
        let id = o
            .get("id")
            .and_then(Value::as_str)
            .ok_or("a node has no `id`")?
            .to_owned();
        let ty = o
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("node {id} has no `type`"))?
            .to_owned();
        let name = o
            .get("name")
            .and_then(Value::as_str)
            .map_or_else(|| default_name(&ty), str::to_owned);
        let children = match o.get("children") {
            Some(Value::Array(a)) => a.iter().map(Self::from_value).collect::<Result<_, _>>()?,
            _ => Vec::new(),
        };
        let props = o
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "id" | "type" | "name" | "children"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Ok(Self {
            id,
            ty,
            name,
            props,
            children,
        })
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.props.get(key)
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }

    pub fn num(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(Value::as_f64)
    }

    pub fn flag(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(Value::as_bool)
    }

    /// `reusable: true` or a variant map.
    pub fn is_reusable(&self) -> bool {
        match self.get("reusable") {
            Some(Value::Bool(b)) => *b,
            Some(Value::Object(_)) => self.ty != "variants",
            _ => false,
        }
    }

    pub fn walk<'a>(&'a self, f: &mut dyn FnMut(&'a PNode)) {
        f(self);
        for c in &self.children {
            c.walk(f);
        }
    }
}

fn default_name(ty: &str) -> String {
    let mut c = ty.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// A variable value, for every theme or for one theme combination.
#[derive(Debug, Clone, PartialEq)]
pub struct VarValue {
    /// Theme axis → value. Empty: all themes.
    pub theme: BTreeMap<String, String>,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variable {
    pub name: String,
    /// `color`, `number`, `string`, `boolean`, …
    pub kind: String,
    pub values: Vec<VarValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub version: String,
    pub children: Vec<PNode>,
    /// Theme axes and their values, in source order.
    pub themes: Vec<(String, Vec<String>)>,
    pub variables: Vec<Variable>,
    /// Stable identity of the source file, when it has one.
    pub file_token: Option<String>,
    /// Top-level keys this importer does not read.
    pub unknown_keys: Vec<String>,
}

/// `"2.25"` → `(2, 25)`.
pub fn parse_version(v: &str) -> Option<(u32, u32)> {
    let mut it = v.trim().split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next().unwrap_or("0").parse().ok()?;
    Some((major, minor))
}

pub fn parse(text: &str) -> Result<Source, String> {
    let root: Value =
        serde_json::from_str(text).map_err(|e| format!("not a pen.dev document: {e}"))?;
    let Value::Object(o) = &root else {
        return Err("not a pen.dev document: the top level is not an object".into());
    };
    let version = match o.get("version") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => return Err("not a pen.dev document: no `version`".into()),
    };
    let v = parse_version(&version)
        .ok_or_else(|| format!("unreadable pen.dev schema version `{version}`"))?;
    if v < SCHEMA_MIN || v > SCHEMA_MAX {
        return Err(format!(
            "pen.dev schema {version} is not supported; this importer reads {}.{} – {}.{}",
            SCHEMA_MIN.0, SCHEMA_MIN.1, SCHEMA_MAX.0, SCHEMA_MAX.1
        ));
    }
    let children = match o.get("children") {
        Some(Value::Array(a)) => a.iter().map(PNode::from_value).collect::<Result<_, _>>()?,
        _ => Vec::new(),
    };
    let themes = match o.get("themes") {
        Some(Value::Object(t)) => t
            .iter()
            .map(|(axis, vals)| {
                let vals = vals
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                (axis.clone(), vals)
            })
            .collect(),
        _ => Vec::new(),
    };
    let variables = match o.get("variables") {
        Some(Value::Object(vars)) => vars
            .iter()
            .map(|(name, def)| {
                let kind = def
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("string")
                    .to_owned();
                let values = match def.get("value") {
                    Some(Value::Array(a))
                        if a.iter().all(|x| x.is_object() && x.get("value").is_some()) =>
                    {
                        a.iter()
                            .map(|x| VarValue {
                                theme: x
                                    .get("theme")
                                    .and_then(Value::as_object)
                                    .map(|t| {
                                        t.iter()
                                            .filter_map(|(k, v)| {
                                                Some((k.clone(), v.as_str()?.to_owned()))
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                                value: x.get("value").cloned().unwrap_or(Value::Null),
                            })
                            .collect()
                    }
                    Some(v) => vec![VarValue {
                        theme: BTreeMap::new(),
                        value: v.clone(),
                    }],
                    None => Vec::new(),
                };
                Variable {
                    name: name.clone(),
                    kind,
                    values,
                }
            })
            .collect(),
        _ => Vec::new(),
    };
    let file_token = o
        .get("fileToken")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let unknown_keys = o
        .keys()
        .filter(|k| {
            !matches!(
                k.as_str(),
                "version" | "children" | "themes" | "variables" | "fileToken"
            )
        })
        .cloned()
        .collect();
    Ok(Source {
        version,
        children,
        themes,
        variables,
        file_token,
        unknown_keys,
    })
}

/// Nodes by ID, over the whole document.
pub struct Index<'a> {
    pub by_id: HashMap<&'a str, &'a PNode>,
}

impl<'a> Index<'a> {
    pub fn new(roots: &'a [PNode]) -> Self {
        let mut by_id = HashMap::new();
        for r in roots {
            r.walk(&mut |n| {
                by_id.insert(n.id.as_str(), n);
            });
        }
        Self { by_id }
    }

    pub fn get(&self, id: &str) -> Option<&'a PNode> {
        self.by_id.get(id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_versions_and_refuses_others() {
        assert_eq!(parse_version("2.25"), Some((2, 25)));
        assert!(parse(r#"{"version":"2.25","children":[]}"#).is_ok());
        let e = parse(r#"{"version":"3.0","children":[]}"#).unwrap_err();
        assert!(e.contains("3.0") && e.contains("2.14"), "{e}");
        assert!(parse(r#"{"version":"2.13"}"#).is_err());
        assert!(parse("[]").is_err());
    }

    #[test]
    fn reads_variables_per_theme() {
        let s = parse(
            r##"{"version":"2.20","themes":{"mode":["light","dark"]},
            "variables":{"bg":{"type":"color","value":[{"value":"#fff","theme":{"mode":"light"}},{"value":"#000","theme":{"mode":"dark"}}]},
            "r":{"type":"number","value":4}},"extra":1}"##,
        )
        .unwrap();
        assert_eq!(
            s.themes,
            vec![("mode".into(), vec!["light".into(), "dark".into()])]
        );
        let bg = s.variables.iter().find(|v| v.name == "bg").unwrap();
        assert_eq!(bg.values.len(), 2);
        assert_eq!(bg.values[1].theme["mode"], "dark");
        let r = s.variables.iter().find(|v| v.name == "r").unwrap();
        assert_eq!(r.values[0].value, serde_json::json!(4));
        assert_eq!(s.unknown_keys, vec!["extra".to_string()]);
    }
}
