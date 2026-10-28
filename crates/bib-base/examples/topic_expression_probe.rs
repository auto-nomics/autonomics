//! Compare qrels-free English query ASTs with the frozen whitespace-OR policy.
use bib_types::SearchExpr;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 2 {
        return Err("usage: topic_expression_probe QUERY_JSONL EXPECTED_QUERY_COUNT".into());
    }
    let expected: usize = args[1].parse()?;
    let mut count = 0;
    for line in BufReader::new(std::fs::File::open(&args[0])?).lines() {
        let row: Value = serde_json::from_str(&line?)?;
        let text = row["text"].as_str().ok_or("missing query text")?;
        let previous = SearchExpr::Or {
            clauses: text
                .split_whitespace()
                .filter(|word| word.chars().any(char::is_alphanumeric))
                .map(|value| SearchExpr::Term {
                    field: None,
                    value: value.into(),
                })
                .collect(),
        };
        if bib_base::topic_search_expression(text)? != previous {
            return Err(format!("query {} changed its frozen expression", row["_id"]).into());
        }
        count += 1;
    }
    if count == 0 || count != expected {
        return Err("unexpected query count".into());
    }
    println!(
        "{}",
        json!({"schema":"bib-topic-expression-compatibility-v1","queries":count,"all_expressions_identical":true,"judgments_read":false,
        "query_sha256":format!("{:x}",Sha256::digest(std::fs::read(&args[0])?)),"topic_source_sha256":format!("{:x}",Sha256::digest(include_bytes!("../src/hybrid_search.rs")))})
    );
    Ok(())
}
