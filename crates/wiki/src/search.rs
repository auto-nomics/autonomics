//! BM25 全文检索的纯文本侧:中英文混合分词 + 打分公式。
//!
//! 分词沿用 bib-base `search_analyzer` 的策略(工作区既定形态):非 Han
//! 的字母数字连续段按词边界切分并小写化,Han 连续段交给 jieba(词典随
//! Cargo 锁定,输出确定)。文档与查询共用 [`tokenize`],两侧 token 空间
//! 一致;Han 段用 `cut_for_search`,长词额外产出词典子词,「全基因组」
//! 里的「基因组」也能被「基因组」查到。
//!
//! 打分用 BM25Okapi:k1=1.2、b=0.75(与 text-embedding 的 BM25 默认
//! 一致),idf = ln(1 + (N-df+0.5)/(df+0.5)) 恒正。

use std::sync::LazyLock;

use jieba_rs::Jieba;

static JIEBA: LazyLock<Jieba> = LazyLock::new(Jieba::new);

/// BM25 词频饱和系数(经典 Robertson 值)。
pub const K1: f64 = 1.2;
/// BM25 长度归一化强度。
pub const B: f64 = 0.75;

/// Han 区段判定;范围与 bib-base `search_analyzer::is_han` 相同。
fn is_han(ch: char) -> bool {
    matches!(ch as u32, 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x323af)
}

/// 中英文混合分词:非 Han 字母数字连续段(小写)为一个词,连续 Han
/// 段按 jieba 搜索模式切分;其余字符(标点、空白、markdown 语法)是
/// 分隔符。返回值保持文本顺序。
pub fn tokenize(text: &str) -> Vec<String> {
    // 第一遍:切成 (是否 Han, 连续段) 序列。
    let mut runs: Vec<(bool, &str)> = Vec::new();
    let mut start: Option<usize> = None;
    let mut run_is_han = false;
    for (pos, ch) in text.char_indices() {
        let kind = if is_han(ch) {
            Some(true)
        } else if ch.is_alphanumeric() {
            Some(false)
        } else {
            None
        };
        match kind {
            Some(is_han_kind) => match start {
                None => {
                    start = Some(pos);
                    run_is_han = is_han_kind;
                }
                Some(_) if is_han_kind == run_is_han => {}
                Some(s) => {
                    runs.push((run_is_han, &text[s..pos]));
                    start = Some(pos);
                    run_is_han = is_han_kind;
                }
            },
            None => {
                if let Some(s) = start.take() {
                    runs.push((run_is_han, &text[s..pos]));
                }
            }
        }
    }
    if let Some(s) = start {
        runs.push((run_is_han, &text[s..]));
    }

    // 第二遍:Latin 段小写整段成词,Han 段交给 jieba。
    let mut tokens = Vec::new();
    for (han, run) in runs {
        if han {
            for token in JIEBA.cut_for_search(run, true) {
                tokens.push(token.word.to_string());
            }
        } else {
            tokens.push(run.to_lowercase());
        }
    }
    tokens
}

/// BM25Okapi idf:恒正,出现在所有文档里的词趋于 0 而不会变负。
/// `n_docs >= df` 由一致索引保证;f64 计算防御异常输入。
pub fn idf(n_docs: usize, df: usize) -> f64 {
    let n = n_docs as f64;
    let d = df as f64;
    (1.0 + (n - d + 0.5) / (d + 0.5)).ln()
}

/// BM25 词频项:tf 饱和 + 文档长度归一化。
pub fn bm25_term(tf: i64, doc_len: i64, avgdl: f64) -> f64 {
    let tf = tf as f64;
    let dl = doc_len as f64;
    let avgdl = avgdl.max(1e-9);
    tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * dl / avgdl))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin_words_are_lowercased_and_split() {
        assert_eq!(
            tokenize("GWAS study, 2024! (cohort)"),
            vec![
                "gwas".to_string(),
                "study".to_string(),
                "2024".to_string(),
                "cohort".to_string()
            ]
        );
    }

    #[test]
    fn markdown_syntax_is_separator() {
        assert_eq!(
            tokenize("# Hello **wiki**\n- item"),
            vec!["hello".to_string(), "wiki".to_string(), "item".to_string()]
        );
    }

    /// jieba 经典示例句(README 是全量 cut 的输出;搜索模式额外给出
    /// 清华/华大/大学 子词,词典锁定下输出确定)。
    #[test]
    fn han_run_is_cut_by_jieba() {
        assert_eq!(
            tokenize("我来到北京清华大学"),
            vec![
                "我".to_string(),
                "来到".to_string(),
                "北京".to_string(),
                "清华".to_string(),
                "华大".to_string(),
                "大学".to_string(),
                "清华大学".to_string(),
            ]
        );
    }

    /// cut_for_search 在长词外额外产出词典子词。
    #[test]
    fn search_mode_emits_subwords() {
        let tokens = tokenize("中华人民共和国");
        assert!(tokens.contains(&"中华".to_string()));
        assert!(tokens.contains(&"人民".to_string()));
        assert!(tokens.contains(&"中华人民共和国".to_string()));
    }

    #[test]
    fn mixed_text_keeps_runs_separate() {
        let tokens = tokenize("BRCA1基因突变");
        assert_eq!(
            tokens,
            vec![
                "brca1".to_string(),
                "基因".to_string(),
                "突变".to_string(),
                "基因突变".to_string(),
            ]
        );
    }

    #[test]
    fn empty_and_punct_only_text_yield_nothing() {
        assert!(tokenize("").is_empty());
        assert!(tokenize("!!! --- ***").is_empty());
    }

    #[test]
    fn idf_is_positive_and_decreasing_in_df() {
        let rare = idf(10, 1);
        let common = idf(10, 9);
        assert!(rare > common);
        assert!(common > 0.0);
    }

    #[test]
    fn bm25_term_saturates_and_penalizes_length() {
        let short = bm25_term(1, 10, 100.0);
        let long = bm25_term(1, 200, 100.0);
        assert!(short > long);
        // tf 翻倍贡献递减。
        let t1 = bm25_term(1, 100, 100.0);
        let t2 = bm25_term(2, 100, 100.0);
        assert!(t2 > t1);
        assert!(t2 < 2.0 * t1);
    }
}
