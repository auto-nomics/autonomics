//! Retrieve a BEIR corpus through the production bibliography actor and Tantivy index.
//! No qrels are read: query split membership is passed as a separate JSONL file.
//! cargo run --release -p bib-base --example search_relevance -- DATASET QUERIES OUTPUT and|or
use bib_base::{BibBase, SearchEngineMode};
use bib_types::{Article, SearchExpr, SearchRequestV2};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::{BufRead, BufReader, BufWriter, Write},
    path::PathBuf,
    time::Instant,
};

#[derive(Deserialize)]
struct Record {
    #[serde(rename = "_id")]
    id: String,
    #[serde(default)]
    title: String,
    text: String,
}

fn read(path: &std::path::Path) -> Result<Vec<Record>, Box<dyn std::error::Error>> {
    let mut ids = HashSet::new();
    let mut rows = Vec::new();
    for line in BufReader::new(std::fs::File::open(path)?).lines() {
        let row: Record = serde_json::from_str(&line?)?;
        if row.id.is_empty() || !ids.insert(row.id.clone()) {
            return Err("empty or duplicate object ID".into());
        }
        rows.push(row);
    }
    if rows.is_empty() {
        return Err("empty input".into());
    }
    Ok(rows)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 4 || !matches!(args[3].as_str(), "and" | "or") {
        return Err("usage: search_relevance DATASET QUERY_JSONL OUTPUT_JSONL and|or".into());
    }
    let corpus_path = PathBuf::from(&args[0]).join("corpus.jsonl");
    let queries_path = PathBuf::from(&args[1]);
    let corpus = read(&corpus_path)?;
    let queries = read(&queries_path)?;
    let db = BibBase::open_with_search_engine(":memory:", SearchEngineMode::Tantivy).await?;
    let started = Instant::now();
    let documents = corpus.len();
    let articles = corpus
        .into_iter()
        .map(|row| {
            let mut article = Article::new(row.id, row.title);
            article.abstract_text = Some(row.text);
            article
        })
        .collect::<Vec<_>>();
    for batch in articles.chunks(500) {
        db.upsert_articles(batch).await?;
    }
    db.rebuild_search_index().await?;
    let index_ms = started.elapsed().as_secs_f64() * 1000.0;
    let output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[2])?;
    let mut writer = BufWriter::new(output);
    let mut times = Vec::new();
    for row in &queries {
        // Explicit AST avoids interpreting natural scientific claims as query syntax.
        // Whitespace token OR is a diagnostic baseline; retain the original token,
        // so the production analyzer still handles symbols and Chinese text.
        let clauses = row
            .text
            .split_whitespace()
            .filter(|word| word.chars().any(char::is_alphanumeric))
            .map(|word| SearchExpr::Term {
                value: word.to_owned(),
                field: None,
            })
            .collect::<Vec<_>>();
        if clauses.is_empty() {
            return Err("query text is empty".into());
        }
        let expression = if args[3] == "or" {
            SearchExpr::Or { clauses }
        } else {
            SearchExpr::Term {
                value: row.text.clone(),
                field: None,
            }
        };
        let started = Instant::now();
        let page = db
            .search_v2(SearchRequestV2 {
                expression: Some(expression),
                page_size: 100,
                ..Default::default()
            })
            .await?;
        times.push(started.elapsed().as_secs_f64() * 1000.0);
        serde_json::to_writer(
            &mut writer,
            &json!({"query_id":row.id,"hits":page.hits
            .iter().map(|hit|json!({"article_id":hit.article_id,"score":hit.score}))
            .collect::<Vec<_>>() }),
        )?;
        writeln!(writer)?;
    }
    writer.flush()?;
    times.sort_by(f64::total_cmp);
    let hash = |path: &std::path::Path| -> Result<String, std::io::Error> {
        Ok(format!("{:x}", Sha256::digest(std::fs::read(path)?)))
    };
    eprintln!(
        "{}",
        json!({"schema":"bib-lexical-relevance-v1", "documents":documents,
        "queries":queries.len(),"operator":args[3],"page_size":100,"index_ms":index_ms,
        "query_p50_ms":times[times.len()/2],"query_p95_ms":times[(times.len()*95/100).min(times.len()-1)],
        "corpus_sha256":hash(&corpus_path)?,"queries_sha256":hash(&queries_path)?,
        "output_sha256":hash(std::path::Path::new(&args[2]))?,"release":!cfg!(debug_assertions)})
    );
    Ok(())
}
