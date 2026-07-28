//! merge_subgraph —— 合并 per-chrom 子图 Parquet 为一个，加 chrom 字段。
//!
//! precompute_tags 已直接存 rsid（id_a/id_b: Utf8），本工具只做拼接 + 加 chrom。
//! 不读 AF，不做任何 index→rsid 转换。
//!
//! 用法：
//!   cargo run -p data-engine --bin merge_subgraph -- . eur_subgraph.parquet

use std::fs::File;
use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

fn main() -> Result<(), BoxErr> {
    let args: Vec<String> = std::env::args().collect();
    let input_dir = args.get(1).map(|s| s.as_str()).unwrap_or(".");
    let output = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "eur_subgraph.parquet".to_string());

    let out_schema = Arc::new(Schema::new(vec![
        Field::new("chrom", DataType::UInt32, false),
        Field::new("id_a", DataType::Utf8, false),
        Field::new("id_b", DataType::Utf8, false),
        Field::new("r2", DataType::Float32, false),
        Field::new("h_a", DataType::Float32, false),
        Field::new("h_b", DataType::Float32, false),
    ]));

    let out_file = File::create(&output)?;
    let mut writer = ArrowWriter::try_new(out_file, out_schema.clone(), None)?;
    let mut total_rows = 0usize;

    for chrom in 1..=22u32 {
        let path = format!("{input_dir}/chr{chrom}_subgraph.parquet");
        if !std::path::Path::new(&path).exists() {
            eprintln!("[skip] chr{chrom}");
            continue;
        }
        let file = File::open(&path)?;
        let reader = ParquetRecordBatchReaderBuilder::try_new(file)?.build()?;
        let mut n = 0usize;
        for batch in reader {
            let batch = batch?;
            let rows = batch.num_rows();
            // 在最前面插入 chrom 列，原始 5 列原样保留
            let chrom_col = Arc::new(arrow_array::UInt32Array::from_value(chrom, rows)) as _;
            let mut cols = vec![chrom_col];
            cols.extend(batch.columns().iter().cloned());
            let merged = RecordBatch::try_new(out_schema.clone(), cols)?;
            writer.write(&merged)?;
            n += rows;
        }
        total_rows += n;
        eprintln!("[merge] chr{chrom}: {n} 行");
    }

    writer.close()?;
    let size = std::fs::metadata(&output)?.len();
    eprintln!(
        "done: {total_rows} 行, {:.1} MB → {output}",
        size as f64 / 1e6
    );
    Ok(())
}
