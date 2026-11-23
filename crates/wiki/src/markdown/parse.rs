//! 链接提取:wiki 只解释链接语法,且只为填充链接索引 —— 渲染永远
//! 不改写正文。
//!
//! 两类链接:
//! - `Wiki`:`[[名称]]` / `[[名称|显示]]`(Obsidian 风格 pothole),名称按
//!   词条标题或别名解析到词条 id —— 纯名称索引,没有路径语义。
//! - `External`:`[文本](url)` 等标准 markdown 外链,不进链接索引,
//!   提取仅供上层引用统计。
//!
//! 解析用 pulldown-cmark 0.13 的原生 `ENABLE_WIKILINKS` 扩展:代码块与
//! 行内代码天然不产生链接事件,wikilink 的目标在 `dest_url`、显示文本
//! 在链接内部文本事件里。

use pulldown_cmark::{Event, LinkType, Options, Tag, TagEnd};

/// 内链(Wiki)还是外链(External)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    /// `[[目标]]` / `[[目标|显示]]`。
    Wiki,
    /// `[文本](url)`。
    External,
}

/// 单条文献引用键(pandoc 语法 `[@citekey]` / `[@k1; @k2]`)。
///
/// citekey 指向 bib 库的稳定引用键(如 `smith2024a-k3x9`),目标不在
/// wiki entries 表里,所以不走 [`LinkKind`]/链接索引,而是单独的引用
/// 边表(见 storage 的 `entry_citations`)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedCitation {
    /// 引用键原文(`@` 后的部分,已 trim)。
    pub key: String,
}

/// 合法引用键字符:字母数字开头,后接字母数字/下划线/点/连字符
/// (覆盖 `smith2024a-k3x9` 这类 词干-后缀 键)。
fn is_valid_citekey_char(c: char, first: bool) -> bool {
    if first {
        c.is_ascii_alphanumeric() || c == '_'
    } else {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
    }
}

/// 一次链接提取结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedLink {
    pub kind: LinkKind,
    /// Wiki:规范化后的名称(见 [`Parser::normalize_name`]);External:URL
    /// 原文。
    pub target: String,
    /// 显示文本(`|显示` 或 md 链接文本);Wiki 链接与目标相同时为 None。
    pub display: Option<String>,
}

/// Markdown 链接解析器:链接提取与目标规范化收拢在这一个对象上。
///
/// 命名不与 `pulldown_cmark::Parser` 冲突:本模块内外部解析器以全限定
/// 路径调用,`Parser` 留给本类型。当前无状态:实例可复用、可静态化
/// (`Parser::new()` 零开销),保留对象形态是为了日后挂解析配置(如额外
/// 选项、缓存)不用改调用方。
#[derive(Debug, Clone, Copy, Default)]
pub struct Parser {}

impl Parser {
    pub fn new() -> Self {
        Self {}
    }

    /// 规范化内链名称:只去首尾空白。词条按名称(标题或别名)索引,
    /// 没有路径语义 —— `/`、`./` 等前缀原样保留,只匹配字面标题。
    /// 不做大小写折叠(与标题唯一性口径一致)。
    pub fn normalize_name(&self, target: &str) -> String {
        target.trim().to_string()
    }

    /// 从 markdown 内容提取全部链接,保持出现顺序,不去重。
    pub fn extract_links(&self, content: &str) -> Vec<ExtractedLink> {
        let mut links = Vec::new();
        // 打开的链接栈:(种类, 目标, 显示文本缓冲)。
        let mut open: Vec<(LinkKind, String, String)> = Vec::new();

        let parser = pulldown_cmark::Parser::new_ext(content, Options::ENABLE_WIKILINKS);
        for event in parser {
            match event {
                Event::Start(Tag::Link {
                    link_type,
                    dest_url,
                    ..
                }) => {
                    let kind = match link_type {
                        LinkType::WikiLink { .. } => LinkKind::Wiki,
                        _ => LinkKind::External,
                    };
                    open.push((kind, dest_url.into_string(), String::new()));
                }
                Event::End(TagEnd::Link) => {
                    let Some((kind, target, text)) = open.pop() else {
                        continue;
                    };
                    let text = text.trim().to_string();
                    match kind {
                        LinkKind::Wiki => {
                            let target = self.normalize_name(&target);
                            // 跨行目标是解析歧义产物,标题也不可能含换行。
                            if target.is_empty() || target.contains('\n') {
                                continue;
                            }
                            let display = if text.is_empty() || self.normalize_name(&text) == target
                            {
                                None
                            } else {
                                Some(text)
                            };
                            links.push(ExtractedLink {
                                kind,
                                target,
                                display,
                            });
                        }
                        LinkKind::External => {
                            links.push(ExtractedLink {
                                kind,
                                target,
                                display: (!text.is_empty()).then_some(text),
                            });
                        }
                    }
                }
                Event::Text(t) => {
                    if let Some((_, _, buffer)) = open.last_mut() {
                        buffer.push_str(&t);
                    }
                }
                _ => {}
            }
        }
        links
    }

    /// 从 markdown 内容提取全部文献引用键(pandoc 语法),保持出现顺序,
    /// 不去重。
    ///
    /// `[@smith2024a-k3x9]` 与多键组 `[@smith2024a-k3x9; @jones2021ab]`
    /// 都支持;键之间以 `;` 分隔,每键以 `@` 起头。代码块与行内代码里的
    /// 字面 `[@…]` 不提取(与 wikilink 同规则):通过 pulldown-cmark 事件
    /// 流过滤,只对普通文本段做扫描。
    pub fn extract_citations(&self, content: &str) -> Vec<ExtractedCitation> {
        let mut plain = String::with_capacity(content.len());
        let mut in_code_block = false;
        for event in pulldown_cmark::Parser::new_ext(content, Options::ENABLE_WIKILINKS) {
            match event {
                Event::Start(Tag::CodeBlock(_)) => in_code_block = true,
                Event::End(TagEnd::CodeBlock) => in_code_block = false,
                // 行内代码整段自成事件,不需要状态。
                Event::Code(_) | Event::Html(_) | Event::InlineHtml(_) => {}
                Event::Text(t) => {
                    if !in_code_block {
                        plain.push_str(&t);
                    }
                }
                Event::SoftBreak | Event::HardBreak => {
                    if !in_code_block {
                        plain.push('\n');
                    }
                }
                _ => {}
            }
        }
        scan_citations(&plain)
    }

    /// 把正文中目标为 `from` 的内链改写为 `to`(改名时保链接不断)。
    ///
    /// 纯文本扫描 `[[…]]`:目标段(`|` 前、去首尾空白)等于 `from` 才
    /// 改写,显示文本原样保留;跨行的 `[[` 不构成链接,同样跳过。与
    /// [`Parser::extract_links`] 的差异:行内代码中的字面 `[[from]]`
    /// 不会被解析成链接,但这里会被改写 —— 保守取舍,正文极少在行内
    /// 代码里写完整内链。
    pub fn rewrite_wiki_link_target(&self, content: &str, from: &str, to: &str) -> String {
        if from.is_empty() || from == to || !content.contains("[[") {
            return content.to_string();
        }
        let mut out = String::with_capacity(content.len());
        let mut copied = 0usize; // 已拷贝到的字节偏移
        let mut search = 0usize;
        while let Some(rel) = content[search..].find("[[") {
            let start = search + rel;
            let Some(rel_close) = content[start + 2..].find("]]") else {
                break;
            };
            let close = start + 2 + rel_close;
            search = if content[start..close].contains('\n') {
                // 跨行不是链接,从这对括号之后继续找
                close
            } else {
                let inner = &content[start + 2..close];
                let (target, tail) = match inner.find('|') {
                    Some(p) => (&inner[..p], &inner[p..]),
                    None => (inner, ""),
                };
                if self.normalize_name(target) == from {
                    out.push_str(&content[copied..start]);
                    out.push_str("[[");
                    out.push_str(to);
                    out.push_str(tail);
                    out.push_str("]]");
                    copied = close + 2;
                }
                close + 2
            };
        }
        if copied == 0 {
            return content.to_string();
        }
        out.push_str(&content[copied..]);
        out
    }
}

/// 扫描纯文本里的 pandoc 引用组。引用组从 `[@` 开始到最近的 `]`,
/// 组内按 `;` 拆分,每项 trim 后必须 `@` 起头且键字符合法,否则整项
/// 丢弃(宁缺勿滥:普通方括号文本如 `[a@b]` 不该被当引用)。
fn scan_citations(plain: &str) -> Vec<ExtractedCitation> {
    let mut out = Vec::new();
    let bytes = plain.as_bytes();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'@' {
            // 找最近的 `]`;跨行的组仍接受(pandoc 允许换行),但键本身
            // 不含换行。
            let Some(rel) = plain[i + 2..].find(']') else {
                break;
            };
            let group = &plain[i + 2..i + 2 + rel];
            for (n, item) in group.split(';').enumerate() {
                let item = item.trim();
                // The opening `[@` already consumed the first item's `@`;
                // later items must carry their own. A space between `[@` and
                // the key (e.g. `[@ bad]`) is not a citation.
                if n == 0 && !group.starts_with(item) {
                    continue;
                }
                let key = if n == 0 {
                    item.strip_prefix('@').unwrap_or(item)
                } else {
                    match item.strip_prefix('@') {
                        Some(k) => k,
                        None => continue,
                    }
                };
                let mut chars = key.chars();
                match chars.next() {
                    Some(first) if is_valid_citekey_char(first, true) => {}
                    _ => continue,
                }
                if chars.all(|c| is_valid_citekey_char(c, false)) {
                    out.push(ExtractedCitation {
                        key: key.to_string(),
                    });
                }
            }
            i += 2 + rel + 1;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试辅助:一次性解析器。
    fn parse(content: &str) -> Vec<ExtractedLink> {
        Parser::new().extract_links(content)
    }

    fn wiki(target: &str, display: Option<&str>) -> ExtractedLink {
        ExtractedLink {
            kind: LinkKind::Wiki,
            target: target.to_string(),
            display: display.map(str::to_string),
        }
    }

    #[test]
    fn plain_wiki_link() {
        assert_eq!(parse("参见 [[遗传学]] 一节"), vec![wiki("遗传学", None)]);
    }

    #[test]
    fn wiki_link_with_display() {
        assert_eq!(
            parse("见 [[GWAS|全基因组关联分析]]"),
            vec![wiki("GWAS", Some("全基因组关联分析"))]
        );
    }

    #[test]
    fn multiple_links_keep_order() {
        let links = parse("[[A]] 中间 [[B|b]] 结尾 [[A]]");
        assert_eq!(links.len(), 3);
        assert_eq!(links[0].target, "A");
        assert_eq!(links[1].display.as_deref(), Some("b"));
        assert_eq!(links[2].target, "A");
    }

    #[test]
    fn target_is_trimmed_not_path_resolved() {
        // 只 trim;斜杠前缀不剥,按字面名称解析(无路径语义)。
        assert_eq!(parse("[[  Home  ]]"), vec![wiki("Home", None)]);
        assert_eq!(parse("[[/Home]]"), vec![wiki("/Home", None)]);
        assert_eq!(parse("[[./GWAS]]"), vec![wiki("./GWAS", None)]);
    }

    #[test]
    fn empty_target_dropped() {
        assert!(parse("[[]] [[ |x ]] [[  ]]").is_empty());
    }

    #[test]
    fn code_blocks_are_ignored() {
        let content = "```\n[[不应提取]]\n```\n正文 [[应提取]]";
        assert_eq!(parse(content), vec![wiki("应提取", None)]);
    }

    #[test]
    fn inline_code_is_not_a_link() {
        // `[[` 与 `]]` 在行内代码里,不构成链接。
        assert!(parse("a `[[` b `]]` c").is_empty());
    }

    #[test]
    fn multiline_target_dropped() {
        assert!(parse("[[跨\n行]]").is_empty());
    }

    #[test]
    fn external_links_captured() {
        let links = parse("见 [官方文档](https://example.com/docs)");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].kind, LinkKind::External);
        assert_eq!(links[0].target, "https://example.com/docs");
        assert_eq!(links[0].display.as_deref(), Some("官方文档"));
    }

    #[test]
    fn headings_and_lists_still_scanned() {
        let content = "# 标题 [[H]]\n\n- 条目 [[L]]";
        assert_eq!(parse(content), vec![wiki("H", None), wiki("L", None)]);
    }

    #[test]
    fn normalize_name_trims_only() {
        let parser = Parser::new();
        assert_eq!(parser.normalize_name("  Home  "), "Home");
        // 前缀与中间斜杠原样保留:名称就是标题,没有路径解析。
        assert_eq!(parser.normalize_name("/a/b "), "/a/b");
        assert_eq!(parser.normalize_name("./x"), "./x");
    }

    #[test]
    fn parser_is_reusable_and_default_constructible() {
        let parser = Parser::default();
        assert_eq!(
            parser.extract_links("[[A]] [[B]]"),
            vec![wiki("A", None), wiki("B", None)]
        );
        // 同一实例可连续使用。
        assert_eq!(parser.extract_links("[[C]]"), vec![wiki("C", None)]);
    }

    #[test]
    fn rewrite_replaces_targets_only() {
        let p = Parser::new();
        assert_eq!(
            p.rewrite_wiki_link_target("[[A]] 见 [[A|甲]] 与 [[B]]", "A", "甲乙"),
            "[[甲乙]] 见 [[甲乙|甲]] 与 [[B]]"
        );
        // 首尾空白参与匹配,改写后规范化。
        assert_eq!(
            p.rewrite_wiki_link_target("x [[  A  |a]] y", "A", "N"),
            "x [[N|a]] y"
        );
        // 其它目标的子串不受影响。
        assert_eq!(
            p.rewrite_wiki_link_target("[[AB]] [[BA]]", "A", "N"),
            "[[AB]] [[BA]]"
        );
        // 无匹配原样返回(同一 String 内容)。
        assert_eq!(p.rewrite_wiki_link_target("[[B]]", "A", "N"), "[[B]]");
        assert_eq!(p.rewrite_wiki_link_target("无链接", "A", "N"), "无链接");
    }

    #[test]
    fn rewrite_skips_crossline_and_keeps_display() {
        let p = Parser::new();
        // 跨行 `[[` 不是链接,不改写。
        assert_eq!(
            p.rewrite_wiki_link_target("a [[A\nB]] c", "A", "N"),
            "a [[A\nB]] c"
        );
        // 显示文本保留原样。
        assert_eq!(
            p.rewrite_wiki_link_target("[[A|旧显示]]", "A", "N"),
            "[[N|旧显示]]"
        );
        // from == to 直接短路。
        assert_eq!(p.rewrite_wiki_link_target("[[A]]", "A", "A"), "[[A]]");
    }

    #[test]
    fn single_pandoc_citation() {
        let cites = Parser::new().extract_citations("结论见 [@smith2024a-k3x9]。");
        assert_eq!(cites.len(), 1);
        assert_eq!(cites[0].key, "smith2024a-k3x9");
    }

    #[test]
    fn multi_key_group() {
        let cites = Parser::new().extract_citations("[@smith2024a-k3x9; @jones2021.ab] 复核");
        assert_eq!(
            cites.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(),
            vec!["smith2024a-k3x9", "jones2021.ab"]
        );
    }

    #[test]
    fn code_blocks_and_inline_code_ignored() {
        let content = "```\n[@inblock]\n```\n行内 `[@inline]` 与真引用 [@real2020key]";
        let cites = Parser::new().extract_citations(content);
        assert_eq!(cites.len(), 1);
        assert_eq!(cites[0].key, "real2020key");
    }

    #[test]
    fn plain_brackets_not_citations() {
        // 不以 @ 起头的方括号、键字符合法但前缀不对的,全部忽略。
        assert!(
            Parser::new()
                .extract_citations("[普通] [@ bad] [a@b] [@]")
                .is_empty()
        );
    }

    #[test]
    fn citations_keep_order_and_allow_repeats() {
        let cites = Parser::new().extract_citations("[@a2020x] 中 [@b2021y] 重复 [@a2020x]");
        assert_eq!(cites.len(), 3);
        assert_eq!(cites[2].key, "a2020x");
    }

    #[test]
    fn semicolon_items_require_at_prefix() {
        let cites = Parser::new().extract_citations("[@good2020key; bad; @also-good]");
        assert_eq!(
            cites.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(),
            vec!["good2020key", "also-good"]
        );
    }
}
