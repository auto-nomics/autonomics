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
}
