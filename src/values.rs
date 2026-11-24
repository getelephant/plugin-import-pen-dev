//! Property values: colors, sizes, padding, weights, and variables
//! (`$name`) resolved per theme mode.

use std::collections::HashMap;

use elephant_document::layout::Padding;
use serde_json::Value;

use crate::source::{Source, Variable};

/// `#rgb`, `#rgba`, `#rrggbb`, or `#rrggbbaa` → lowercase `#rrggbb` /
/// `#rrggbbaa` (opaque alpha dropped).
pub fn color(s: &str) -> Option<String> {
    let hex = s.trim().strip_prefix('#')?;
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let full: String = match hex.len() {
        3 | 4 => hex.chars().flat_map(|c| [c, c]).collect(),
        6 | 8 => hex.to_owned(),
        _ => return None,
    };
    let full = full.to_ascii_lowercase();
    if full.len() == 8 && full.ends_with("ff") {
        return Some(format!("#{}", &full[..6]));
    }
    Some(format!("#{full}"))
}

/// Multiplies the alpha of a `#rrggbb(aa)` color by `opacity`.
pub fn with_opacity(c: &str, opacity: f64) -> String {
    if opacity >= 1.0 {
        return c.to_owned();
    }
    let base = &c[..7];
    let a = if c.len() == 9 {
        u8::from_str_radix(&c[7..9], 16).map_or(1.0, |a| f64::from(a) / 255.0)
    } else {
        1.0
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let v = (a * opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("{base}{v:02x}")
}

/// How a node is sized along one axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Size {
    Fixed(f64),
    /// `fill_container`, with an optional fallback size.
    Fill(Option<f64>),
    /// `fit_content`, with an optional fallback size.
    Hug(Option<f64>),
    Unset,
}

pub fn size(v: Option<&Value>) -> Size {
    match v {
        Some(Value::Number(n)) => n.as_f64().map_or(Size::Unset, Size::Fixed),
        Some(Value::String(s)) => {
            let (word, arg) = match s.split_once('(') {
                Some((w, rest)) => (w, rest.trim_end_matches(')').trim().parse::<f64>().ok()),
                None => (s.as_str(), None),
            };
            match word.trim() {
                "fill_container" => Size::Fill(arg),
                "fit_content" => Size::Hug(arg),
                _ => Size::Unset,
            }
        }
        _ => Size::Unset,
    }
}

/// CSS-style padding: a number, or 1–4 numbers.
pub fn padding(v: &Value, num: &mut dyn FnMut(&Value) -> Option<f64>) -> Option<Padding> {
    let all = |x: f64| Padding::uniform(x);
    match v {
        Value::Array(a) => {
            let n: Vec<f64> = a.iter().map(&mut *num).collect::<Option<_>>()?;
            Some(match n.as_slice() {
                [x] => all(*x),
                [v, h] => Padding {
                    top: *v,
                    right: *h,
                    bottom: *v,
                    left: *h,
                },
                [t, h, b] => Padding {
                    top: *t,
                    right: *h,
                    bottom: *b,
                    left: *h,
                },
                [t, r, b, l, ..] => Padding {
                    top: *t,
                    right: *r,
                    bottom: *b,
                    left: *l,
                },
                [] => Padding::default(),
            })
        }
        other => num(other).map(all),
    }
}

/// `"normal"`, `"bold"`, `"600"`, or a number → CSS weight.
pub fn font_weight(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => match s.as_str() {
            "normal" | "regular" => Some(400.0),
            "bold" => Some(700.0),
            "thin" => Some(100.0),
            "light" => Some(300.0),
            "medium" => Some(500.0),
            "semibold" => Some(600.0),
            "black" => Some(900.0),
            other => other.parse().ok(),
        },
        _ => None,
    }
}

/// Variables, resolved per mode of the first theme axis.
pub struct Vars {
    by_name: HashMap<String, Variable>,
    /// The theme axis that becomes token modes.
    pub axis: Option<String>,
    /// Mode names (values of `axis`); the first is the default.
    pub modes: Vec<String>,
    /// Other theme axes, which have no Elephant equivalent.
    pub other_axes: Vec<String>,
}

impl Vars {
    pub fn new(src: &Source) -> Self {
        let axis = src.themes.first().map(|(a, _)| a.clone());
        let modes = src
            .themes
            .first()
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| vec!["default".to_owned()]);
        Self {
            by_name: src
                .variables
                .iter()
                .map(|v| (v.name.clone(), v.clone()))
                .collect(),
            axis,
            modes,
            other_axes: src.themes.iter().skip(1).map(|(a, _)| a.clone()).collect(),
        }
    }

    pub fn default_mode(&self) -> &str {
        &self.modes[0]
    }

    pub fn get(&self, name: &str) -> Option<&Variable> {
        self.by_name.get(name)
    }

    /// The value of variable `name` in `mode`: the entry for that mode, an
    /// entry for every theme, or the first entry.
    pub fn value(&self, name: &str, mode: &str) -> Option<&Value> {
        let v = self.by_name.get(name)?;
        let axis = self.axis.as_deref();
        v.values
            .iter()
            .find(|x| axis.is_some_and(|a| x.theme.get(a).is_some_and(|m| m == mode)))
            .or_else(|| v.values.iter().find(|x| x.theme.is_empty()))
            .or_else(|| v.values.first())
            .map(|x| &x.value)
    }

    /// Follows `$name` references (variables may alias others).
    pub fn resolve<'a>(&'a self, v: &'a Value, mode: &str) -> (Option<&'a Value>, Option<String>) {
        let mut cur = v;
        let mut first = None;
        for _ in 0..16 {
            match cur.as_str().and_then(|s| s.strip_prefix('$')) {
                Some(name) => {
                    if first.is_none() {
                        first = Some(name.to_owned());
                    }
                    match self.value(name, mode) {
                        Some(next) => cur = next,
                        None => return (None, first),
                    }
                }
                None => return (Some(cur), first),
            }
        }
        (None, first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_colors_and_sizes() {
        assert_eq!(color("#FFF").as_deref(), Some("#ffffff"));
        assert_eq!(color("#100D240F").as_deref(), Some("#100d240f"));
        assert_eq!(color("#112233FF").as_deref(), Some("#112233"));
        assert_eq!(color("red"), None);
        assert_eq!(with_opacity("#112233", 0.5), "#11223380");
        assert_eq!(
            size(Some(&serde_json::json!("fill_container"))),
            Size::Fill(None)
        );
        assert_eq!(
            size(Some(&serde_json::json!("fit_content(120)"))),
            Size::Hug(Some(120.0))
        );
        assert_eq!(size(Some(&serde_json::json!(12))), Size::Fixed(12.0));
        let p = padding(&serde_json::json!([4, 8]), &mut |v| v.as_f64()).unwrap();
        assert_eq!((p.top, p.right, p.bottom, p.left), (4.0, 8.0, 4.0, 8.0));
        assert_eq!(font_weight(&serde_json::json!("600")), Some(600.0));
    }
}
