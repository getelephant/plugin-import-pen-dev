//! Icon fonts as SVG assets. pen.dev places Lucide icons by name; the
//! importer ships the Lucide set (ISC license, `data/LUCIDE_LICENSE`) and
//! writes one SVG per icon and color.

use std::collections::HashMap;

use serde::Deserialize;

#[derive(Deserialize)]
pub struct Lucide {
    icons: HashMap<String, String>,
    aliases: HashMap<String, String>,
}

impl Lucide {
    pub fn load() -> Result<Self, String> {
        serde_json::from_str(include_str!("../data/lucide.json"))
            .map_err(|e| format!("the bundled Lucide icons cannot be read: {e}"))
    }

    fn body(&self, name: &str) -> Option<&str> {
        self.icons
            .get(name)
            .or_else(|| self.aliases.get(name).and_then(|n| self.icons.get(n)))
            .map(String::as_str)
    }

    /// The icon as a 24×24 SVG stroked in `color` (`#rrggbb` or
    /// `#rrggbbaa`), or `None` for an unknown name.
    pub fn svg(&self, name: &str, color: &str) -> Option<String> {
        let body = self.body(name)?;
        let (rgb, opacity) = split_alpha(color);
        let opacity = opacity
            .map(|a| format!(" stroke-opacity=\"{a}\""))
            .unwrap_or_default();
        Some(format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"24\" height=\"24\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"{rgb}\"{opacity} stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\">{body}</svg>\n"
        ))
    }
}

/// `#rrggbbaa` → (`#rrggbb`, opacity); other colors pass through.
fn split_alpha(color: &str) -> (String, Option<String>) {
    if color.len() == 9
        && let Ok(a) = u8::from_str_radix(&color[7..9], 16)
    {
        if a == 255 {
            return (color[..7].to_owned(), None);
        }
        let v = (f64::from(a) / 255.0 * 1000.0).round() / 1000.0;
        return (color[..7].to_owned(), Some(format!("{v}")));
    }
    (color.to_owned(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_known_icons_and_aliases() {
        let l = Lucide::load().unwrap();
        let svg = l.svg("circle-check", "#7b2ff780").unwrap();
        assert!(svg.contains("stroke=\"#7b2ff7\""));
        assert!(svg.contains("stroke-opacity=\"0.502\""));
        assert!(svg.contains("<circle"));
        assert!(l.svg("check-circle-2", "#000000").is_some());
        assert!(l.svg("no-such-icon", "#000000").is_none());
    }
}
