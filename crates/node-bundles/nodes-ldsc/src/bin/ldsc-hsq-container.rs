//! Standalone h² runner for the catalog-backed containerized LDSC node.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::{Float64Array, Int64Array};
use arrow_schema::{DataType, Field, Schema};
use datafusion::common::config::CsvOptions;
use datafusion::dataframe::DataFrameWriteOptions;
use datafusion::prelude::{CsvReadOptions, SessionContext};
use ldsc::hsq::{HsqColumns, HsqResult};
use nodes_ldsc::ldsc_common::{quote_table, read_m_5_50, register_listing_table};

#[derive(Debug)]
struct Args {
    input: PathBuf,
    output: PathBuf,
    ld_panel: PathBuf,
    m_panel: PathBuf,
    n_blocks: usize,
    intercept: Option<f64>,
}

fn parse_args() -> Result<Args, String> {
    let mut values = std::env::args().skip(1);
    let mut input = None;
    let mut output = None;
    let mut ld_panel = None;
    let mut m_panel = None;
    let mut n_blocks = None;
    let mut intercept = None;

    while let Some(key) = values.next() {
        let value = values
            .next()
            .ok_or_else(|| format!("missing value for `{key}`"))?;
        match key.as_str() {
            "--input" => input = Some(PathBuf::from(value)),
            "--output" => output = Some(PathBuf::from(value)),
            "--ld-panel" => ld_panel = Some(PathBuf::from(value)),
            "--m-panel" => m_panel = Some(PathBuf::from(value)),
            "--n-blocks" => {
                n_blocks = Some(
                    value
                        .parse()
                        .map_err(|_| format!("invalid n-blocks `{value}`"))?,
                )
            }
            "--intercept" => {
                intercept = Some(
                    value
                        .parse()
                        .map_err(|_| format!("invalid intercept `{value}`"))?,
                )
            }
            _ => return Err(format!("unknown argument `{key}`")),
        }
    }

    Ok(Args {
        input: input.ok_or("--input is required")?,
        output: output.ok_or("--output is required")?,
        ld_panel: ld_panel.ok_or("--ld-panel is required")?,
        m_panel: m_panel.ok_or("--m-panel is required")?,
        n_blocks: n_blocks.ok_or("--n-blocks is required")?,
        intercept,
    })
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("ldsc-hsq-container: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let args = parse_args()?;
    validate_existing_file(&args.input)?;
    validate_existing_path(&args.ld_panel)?;
    validate_existing_path(&args.m_panel)?;
    if args.n_blocks == 0 {
        return Err("--n-blocks must be greater than zero".into());
    }

    let ctx = SessionContext::new();
    let sumstats = ctx
        .read_csv(
            args.input.to_str().ok_or("input path is not UTF-8")?,
            CsvReadOptions::default(),
        )
        .await
        .map_err(|error| format!("read input CSV: {error}"))?;
    ctx.register_table("sumstats", sumstats.into_view())
        .map_err(|error| format!("register input CSV: {error}"))?;
    register_listing_table(&ctx, "ld_panel", &local_url(&args.ld_panel))
        .await
        .map_err(|error| format!("register LD panel: {error}"))?;
    register_listing_table(&ctx, "ld_panel_m", &local_url(&args.m_panel))
        .await
        .map_err(|error| format!("register M panel: {error}"))?;

    let m = read_m_5_50(&ctx, "ld_panel_m", 1)
        .await
        .map_err(|error| format!("read M panel: {error}"))?;
    let joined = ctx
        .sql(&format!(
            r#"SELECT s."z" AS "z", s."n" AS "n",
                      l."ld_score" AS "ref_ld", l."w_ld" AS "w_ld"
               FROM sumstats AS s
               INNER JOIN {} AS l ON s."rsid" = l."rsid"
               ORDER BY l."locus"."position""#,
            quote_table("ld_panel")
        ))
        .await
        .map_err(|error| format!("join sumstats with LD panel: {error}"))?;
    let result = ldsc::hsq::estimate_h2(
        joined,
        HsqColumns {
            snp: "",
            z: "z",
            n: "n",
            ref_ld: vec!["ref_ld"],
            w_ld: "w_ld",
        },
        &m,
        args.n_blocks,
        args.intercept,
    )
    .await
    .map_err(|error| format!("estimate h2: {error}"))?;

    write_result(&ctx, &args.output, &result).await
}

fn local_url(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn validate_existing_file(path: &Path) -> Result<(), String> {
    path.is_file()
        .then_some(())
        .ok_or_else(|| format!("input does not exist: {}", path.display()))
}

fn validate_existing_path(path: &Path) -> Result<(), String> {
    path.exists()
        .then_some(())
        .ok_or_else(|| format!("panel does not exist: {}", path.display()))
}

async fn write_result(
    ctx: &SessionContext,
    output: &Path,
    result: &HsqResult,
) -> Result<(), String> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("h2", DataType::Float64, false),
        Field::new("h2_se", DataType::Float64, false),
        Field::new("intercept", DataType::Float64, true),
        Field::new("intercept_se", DataType::Float64, true),
        Field::new("ratio", DataType::Float64, true),
        Field::new("ratio_se", DataType::Float64, true),
        Field::new("mean_chisq", DataType::Float64, false),
        Field::new("lambda_gc", DataType::Float64, false),
        Field::new("n_snp", DataType::Int64, false),
    ]));
    let batch = arrow_array::RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Float64Array::from(vec![result.h2])),
            Arc::new(Float64Array::from(vec![result.h2_se])),
            Arc::new(Float64Array::from(vec![result.intercept])),
            Arc::new(Float64Array::from(vec![result.intercept_se])),
            Arc::new(Float64Array::from(vec![result.ratio])),
            Arc::new(Float64Array::from(vec![result.ratio_se])),
            Arc::new(Float64Array::from(vec![result.mean_chisq])),
            Arc::new(Float64Array::from(vec![result.lambda_gc])),
            Arc::new(Int64Array::from(vec![result.n_snp as i64])),
        ],
    )
    .map_err(|error| format!("build result batch: {error}"))?;
    let result_df = ctx
        .read_batch(batch)
        .map_err(|error| format!("build result DataFrame: {error}"))?;
    result_df
        .write_csv(
            output.to_str().ok_or("output path is not UTF-8")?,
            DataFrameWriteOptions::new().with_single_file_output(true),
            None::<CsvOptions>,
        )
        .await
        .map_err(|error| format!("write result CSV: {error}"))?;
    Ok(())
}
