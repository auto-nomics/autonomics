//! Client-local LRU over prepared SVG/PNG payloads. Errors are never retained.
use crate::vector::VectorAsset;
use std::collections::VecDeque;
const MAX_ENTRIES: usize = 16;
const MAX_ENCODED_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;
#[derive(Default)]
pub(crate) struct VectorCache {
    entries: VecDeque<(String, VectorAsset, usize)>,
    bytes: usize,
}
impl VectorCache {
    pub(crate) fn get(&mut self, id: &str) -> Option<VectorAsset> {
        let index = self.entries.iter().position(|(key, _, _)| key == id)?;
        let entry = self.entries.remove(index)?;
        let result = entry.1.clone();
        self.entries.push_back(entry);
        Some(result)
    }
    pub(crate) fn insert(&mut self, id: String, value: VectorAsset) {
        let size = value
            .vector
            .data_url
            .len()
            .saturating_add(value.vector.preview_data_url.len());
        if let Some(index) = self.entries.iter().position(|(key, _, _)| key == &id) {
            self.bytes -= self.entries.remove(index).unwrap().2;
        }
        if size > MAX_ENCODED_PAYLOAD_BYTES {
            return;
        }
        while self.entries.len() >= MAX_ENTRIES
            || self.bytes.saturating_add(size) > MAX_ENCODED_PAYLOAD_BYTES
        {
            let Some((_, _, removed)) = self.entries.pop_front() else {
                break;
            };
            self.bytes -= removed;
        }
        self.bytes += size;
        self.entries.push_back((id, value, size));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn vector(id: &str) -> VectorAsset {
        crate::vector::prepare(crate::Asset{id:id.into(),name:id.into(),category:"test".into(),license:"cc-0".into(),license_url:None,author:"test".into(),author_url:None,source_url:"https://example.org".into(),svg_url:"https://example.org/test.svg".into(),catalog_sha256:"catalog".into()},b"<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'><rect width='10' height='10'/></svg>").unwrap()
    }
    #[test]
    fn lru_count_payload_budget_oversized_entries_and_replace_accounting_are_bounded() {
        let mut cache = VectorCache::default();
        for i in 0..MAX_ENTRIES {
            let id = i.to_string();
            cache.insert(id.clone(), vector(&id));
        }
        assert!(cache.get("0").is_some());
        cache.insert("new".into(), vector("new"));
        assert!(cache.get("1").is_none());
        assert!(cache.get("0").is_some());
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        let before = cache.bytes;
        let v = vector("0");
        let size = v.vector.data_url.len() + v.vector.preview_data_url.len();
        let old = cache.entries.iter().find(|e| e.0 == "0").unwrap().2;
        cache.insert("0".into(), v);
        assert_eq!(cache.bytes, before - old + size);
        let mut big = vector("big");
        big.vector.data_url = "x".repeat(MAX_ENCODED_PAYLOAD_BYTES);
        big.vector.preview_data_url.clear();
        cache.insert("big".into(), big);
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.bytes, MAX_ENCODED_PAYLOAD_BYTES);
        let mut oversized = vector("too-big");
        oversized.vector.data_url = "x".repeat(MAX_ENCODED_PAYLOAD_BYTES + 1);
        cache.insert("too-big".into(), oversized);
        assert!(cache.get("too-big").is_none());
        assert_eq!(cache.entries.len(), 1);
    }
}
