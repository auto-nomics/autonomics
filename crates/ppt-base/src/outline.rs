//! Hierarchical slide outline and section membership shared by every client.
use serde::Serialize;
use ppt_types::SlideSpec;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineEntry {
    /// 1-based visible slide number, matching the viewer/exporter.
    pub slide: usize,
    pub title: String,
    pub level: u8,
    pub depth: usize,
    pub parent: Option<usize>,
    pub is_section: bool,
    pub section_title: String,
    pub section_path: Vec<usize>,
    pub hidden: bool,
}

/// Build a tree over visible slides. Cover pages do not become ancestors;
/// section dividers default to level 1, while ordinary pages use the first
/// Markdown heading's level (or explicit frontmatter `level`).
pub fn build_outline(slides: &[SlideSpec]) -> Vec<OutlineEntry> {
    let mut entries: Vec<OutlineEntry> = Vec::with_capacity(slides.len());
    let mut stack: Vec<usize> = Vec::new();
    let mut section_flags: Vec<Option<bool>> = Vec::new();
    for slide in slides {
        let fm = &slide.frontmatter;
        let layout = fm.layout.as_ref().map(|l| l.as_str());
        let cover = layout == Some("cover") || (slide.index == 0 && layout.is_none());
        let section = fm.extras.get("section").and_then(serde_json::Value::as_bool);
        let divider = layout == Some("section") || section == Some(true);
        // Re-read the raw page so existing parsed caches gain heading-level
        // fixes immediately without requiring a Markdown edit.
        let parsed_level = crate::parser::parse_slide_raw(slide.raw.clone(), slide.index).level;
        let mut level = fm.level.or_else(|| if divider { Some(1) } else { parsed_level.or(slide.level) }).unwrap_or(2).clamp(1, 6);
        // In research decks each slide often starts with `#` even inside a
        // chapter. A marked divider owns subsequent ordinary pages until the
        // next divider (or an explicit level says otherwise).
        if !cover && !divider && fm.level.is_none() {
            if let Some(&parent) = stack.iter().rev().find(|&&i| entries[i].is_section) {
                level = level.max(entries[parent].level.saturating_add(1).min(6));
            }
        }
        if !cover {
            while stack.last().is_some_and(|&i| entries[i].level >= level) { stack.pop(); }
        } else { stack.clear(); }
        let parent = if cover { None } else { stack.last().map(|&i| entries[i].slide) };
        let title = slide.title.clone().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| format!("第 {} 页", slide.no()));
        let section_title = fm.extras.get("sectionTitle").and_then(serde_json::Value::as_str).filter(|v| !v.trim().is_empty()).unwrap_or(&title).to_string();
        entries.push(OutlineEntry { slide: slide.no(), title, level, depth: stack.len(), parent, is_section: divider && !cover && section != Some(false), section_title, section_path: Vec::new(), hidden: fm.hide_in_toc.unwrap_or(false) });
        section_flags.push(if cover { Some(false) } else { section });
        if !cover { stack.push(entries.len() - 1); }
    }
    // Any heading with a child naturally forms a section unless opted out.
    for index in 0..entries.len() {
        if let Some(parent) = entries[index].parent {
            if let Some(parent_index) = entries.iter().position(|item| item.slide == parent) {
                if section_flags[parent_index] != Some(false) { entries[parent_index].is_section = true; }
            }
        }
    }
    for index in 0..entries.len() {
        let mut ancestors = Vec::new();
        let mut cursor = Some(entries[index].slide);
        while let Some(no) = cursor {
            let Some(entry) = entries.iter().find(|entry| entry.slide == no) else { break };
            if entry.is_section && !entry.hidden { ancestors.push(no); }
            cursor = entry.parent;
        }
        ancestors.reverse();
        entries[index].section_path = ancestors;
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sections_nested_titles_and_hidden_entries() {
        let md = "---\ntitle: Report\n---\n\n# Report\n\n---\nlayout: section\nsectionTitle: 方法\n---\n\n## 研究方法\n\n---\n\n## 队列\n\n---\nlayout: section\nlevel: 2\n---\n\n### 子章节\n\n---\n\n### 分析\n\n---\nlayout: section\n---\n\n# 结果\n\n---\nhideInToc: true\n---\n\n## 隐藏目录页面\n";
        let deck = crate::deck::assemble(md, "d", "", "");
        let entries = build_outline(&deck.slides);
        assert_eq!(entries.len(), 7);
        assert_eq!(entries[0].parent, None);
        assert!(!entries[0].is_section);
        assert_eq!(entries[1].level, 1);
        assert_eq!(entries[1].section_title, "方法");
        assert_eq!(entries[2].parent, Some(2));
        assert_eq!(entries[3].parent, Some(2));
        assert_eq!(entries[4].section_path, vec![2, 4]);
        assert_eq!(entries[5].parent, None);
        assert_eq!(entries[6].section_path, vec![6]);
        assert!(entries[6].hidden);
    }
    #[test]
    fn heading_parents_are_inferred_and_opt_out_is_respected() {
        let md = "# Cover\n\n---\n\n# Background\n\n---\n\n## Evidence\n\n---\nsection: false\n---\n\n# Appendix\n\n---\n\n## Details\n";
        let entries = build_outline(&crate::deck::assemble(md, "d", "", "").slides);
        assert!(entries[1].is_section);
        assert_eq!(entries[2].section_path, vec![2]);
        assert!(!entries[3].is_section);
        assert!(entries[4].section_path.is_empty());
    }

    #[test]
    fn explicit_divider_groups_same_level_slide_titles_until_next_section() {
        let md = "# Cover\n\n---\nlayout: section\n---\n\n# Methods\n\n---\n\n# Cohort\n\n---\nlayout: section\nlevel: 2\n---\n\n# Analysis\n\n---\n\n# Model\n\n---\nlayout: section\n---\n\n# Results\n\n---\n\n# Outcome\n";
        let entries = build_outline(&crate::deck::assemble(md, "d", "", "").slides);
        assert_eq!(entries[2].section_path, vec![2]);
        assert_eq!(entries[4].section_path, vec![2, 4]);
        assert_eq!(entries[6].section_path, vec![6]);
    }
}
