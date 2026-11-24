//! Deterministic Elephant IDs. Every ID is derived from the source file's
//! identity and a key (`node:<pen id>`, `token:<name>`, …), so importing
//! the same file twice yields the same IDs and a re-import updates the
//! converted files in place. A previous ID map wins over derivation, so
//! IDs survive even if the derivation changes.

use std::collections::BTreeMap;

use elephant_domain::{
    AssetId, ComponentId, FileId, ModeId, NodeId, PageId, TokenId, VariantAxisId, VariantValueId,
};

pub struct Ids {
    salt: String,
    previous: BTreeMap<String, String>,
    /// Every ID handed out, by key.
    pub used: BTreeMap<String, String>,
}

fn fnv(seed: u64, parts: &[&[u8]]) -> u64 {
    let mut h = seed;
    for p in parts {
        for b in p.iter().chain(&[0xff]) {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

fn uuid_string(v: u128) -> String {
    let h = format!("{v:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn parse_uuid(s: &str) -> Option<u128> {
    let hex: String = s.chars().filter(|c| *c != '-').collect();
    (hex.len() == 32)
        .then(|| u128::from_str_radix(&hex, 16).ok())
        .flatten()
}

impl Ids {
    pub fn new(salt: &str, previous: BTreeMap<String, String>) -> Self {
        Self {
            salt: salt.to_owned(),
            previous,
            used: BTreeMap::new(),
        }
    }

    /// The UUID for `key`: from the previous map, or derived (UUID v8).
    pub fn uuid(&mut self, key: &str) -> u128 {
        if let Some(s) = self.used.get(key)
            && let Some(v) = parse_uuid(s)
        {
            return v;
        }
        let v = self
            .previous
            .get(key)
            .and_then(|s| parse_uuid(s))
            .unwrap_or_else(|| {
                let a = fnv(
                    0xcbf2_9ce4_8422_2325,
                    &[self.salt.as_bytes(), key.as_bytes()],
                );
                let b = fnv(
                    0x6c62_272e_07bb_0142,
                    &[key.as_bytes(), self.salt.as_bytes()],
                );
                let v = (u128::from(a) << 64) | u128::from(b);
                // Version 8 (custom), RFC 4122 variant.
                (v & !(0xf << 76) | (0x8 << 76)) & !(0x3 << 62) | (0x2 << 62)
            });
        self.used.insert(key.to_owned(), uuid_string(v));
        v
    }

    pub fn node(&mut self, key: &str) -> NodeId {
        NodeId::from_u128(self.uuid(&format!("node:{key}")))
    }
    pub fn component(&mut self, key: &str) -> ComponentId {
        ComponentId::from_u128(self.uuid(&format!("component:{key}")))
    }
    pub fn page(&mut self, key: &str) -> PageId {
        PageId::from_u128(self.uuid(&format!("page:{key}")))
    }
    pub fn file(&mut self, key: &str) -> FileId {
        FileId::from_u128(self.uuid(&format!("file:{key}")))
    }
    pub fn token(&mut self, key: &str) -> TokenId {
        TokenId::from_u128(self.uuid(&format!("token:{key}")))
    }
    pub fn mode(&mut self, key: &str) -> ModeId {
        ModeId::from_u128(self.uuid(&format!("mode:{key}")))
    }
    pub fn axis(&mut self, key: &str) -> VariantAxisId {
        VariantAxisId::from_u128(self.uuid(&format!("axis:{key}")))
    }
    pub fn value(&mut self, key: &str) -> VariantValueId {
        VariantValueId::from_u128(self.uuid(&format!("value:{key}")))
    }
    pub fn asset(&mut self, key: &str) -> AssetId {
        AssetId::from_u128(self.uuid(&format!("asset:{key}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_and_prefer_the_previous_map() {
        let mut a = Ids::new("file", BTreeMap::new());
        let mut b = Ids::new("file", BTreeMap::new());
        assert_eq!(a.node("x"), b.node("x"));
        assert_ne!(a.node("x"), a.node("y"));
        let s = a.node("x").to_string();
        assert_eq!(s.len(), 36);
        assert_eq!(&s[14..15], "8");
        let prev = BTreeMap::from([(
            "node:x".to_string(),
            "00000000-0000-4000-8000-000000000001".to_string(),
        )]);
        let mut c = Ids::new("other", prev);
        assert_eq!(
            c.node("x"),
            NodeId::from_u128(1 | (0x4 << 76) | (0x8 << 60))
        );
    }
}
