//! lake_cli —— 数据湖命令行工具：查询 / 管理 / 上传。
//!
//! 子命令：
//!   lake_cli query "<sql>"              跑任意 SQL，打印结果
//!   lake_cli sql-file <path>            从文件读 SQL 执行
//!   lake_cli namespaces                 列出根命名空间
//!   lake_cli tables                     递归列出所有表（namespace.table）
//!   lake_cli tables-in <ns>             列出某命名空间下的表（ns 用点分，如 gwas / mixer）
//!   lake_cli schema <ident>             查看表结构（列名/类型）
//!   lake_cli count <ident>              行数（SELECT COUNT(*)）
//!   lake_cli drop <ident>               删表
//!   lake_cli upload <file> <ident>      上传 CSV/Parquet 文件到 Iceberg 表
//!                  [--csv|--parquet]    （不指定则按扩展名推断）
//!                  [--overwrite]        （默认 append；overwrite 先 drop 再建）
//!
//! <ident> 形如 `gwas.bmi_ieu_a_2`（最后一段是表名，前面是命名空间）。
//!
//! 运行示例：
//!   cargo run -p data-engine --bin lake_cli -- tables
//!   cargo run -p data-engine --bin lake_cli -- count gwas.bmi_ieu_a_2
//!   cargo run -p data-engine --bin lake_cli -- query "SELECT chrom, COUNT(*) FROM iceberg.af.eur_af GROUP BY chrom"
//!   cargo run -p data-engine --bin lake_cli -- upload /path/x.csv mixer.test --csv --overwrite

use arrow_array::RecordBatch;
use datafusion::prelude::{CsvReadOptions, ParquetReadOptions, SessionContext};
use datalake::Datalake;
use iceberg::arrow::arrow_schema_to_schema_auto_assign_ids;
use iceberg::{Catalog, NamespaceIdent, TableCreation, TableIdent};

type BoxErr = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, BoxErr>;

fn usage() -> ! {
    eprintln!(
        "用法: lake_cli <subcommand> [args]\n\
         子命令: query, sql-file, namespaces, tables, tables-in, schema, count, drop, upload\n\
         详见源码顶部文档。"
    );
    std::process::exit(2);
}

/// 把 `ns1.ns2.table` 拆成 (NamespaceIdent, table_name)。
fn parse_ident(ident: &str) -> Result<(NamespaceIdent, String)> {
    let mut parts: Vec<String> = ident.split('.').map(|s| s.to_string()).collect();
    let table = parts.pop().ok_or_else(|| BoxErr::from("ident 为空"))?;
    if parts.is_empty() {
        return Err(BoxErr::from(format!(
            "ident '{ident}' 缺命名空间（形如 ns.table）"
        )));
    }
    let ns = NamespaceIdent::from_vec(parts).map_err(|e| BoxErr::from(e.to_string()))?;
    Ok((ns, table))
}

/// 打印 RecordBatch 列表为简易表格（列名 + 行）。大数据走 DataFusion 自带 show 更好。
async fn print_df(df: datafusion::dataframe::DataFrame, limit: Option<usize>) -> Result<()> {
    let df = if let Some(n) = limit {
        df.limit(0, Some(n))?
    } else {
        df
    };
    df.show().await.map_err(|e| BoxErr::from(e.to_string()))?;
    Ok(())
}

// ════════════════════════════════════════════════════════════════
// 子命令实现
// ════════════════════════════════════════════════════════════════

async fn cmd_query(ctx: &SessionContext, sql: &str) -> Result<()> {
    let df = ctx.sql(sql).await.map_err(|e| BoxErr::from(e.to_string()))?;
    print_df(df, None).await
}

async fn cmd_sql_file(ctx: &SessionContext, path: &str) -> Result<()> {
    let sql = std::fs::read_to_string(path)?;
    // 支持分号分隔的多条语句（按 ; 切分，跳过空串/纯注释）
    for stmt in sql.split(';') {
        let s = stmt.trim();
        if s.is_empty() || s.starts_with("--") {
            continue;
        }
        println!("── SQL ──\n{s}");
        cmd_query(ctx, s).await?;
    }
    Ok(())
}

async fn cmd_namespaces(datalake: &Datalake) -> Result<()> {
    let catalog = datalake.get_catalog().await?;
    let nss = catalog
        .list_namespaces(None)
        .await
        .map_err(|e| BoxErr::from(e.to_string()))?;
    if nss.is_empty() {
        println!("(无命名空间)");
        return Ok(());
    }
    for ns in nss {
        println!("{}", ns.inner().join("."));
    }
    Ok(())
}

async fn cmd_tables(datalake: &Datalake) -> Result<()> {
    let all = datalake
        .list_all_tables()
        .await
        .map_err(|e| BoxErr::from(e.to_string()))?;
    if all.is_empty() {
        println!("(无表)");
        return Ok(());
    }
    for (ns_parts, table) in all {
        let mut full = ns_parts.clone();
        full.push(table);
        println!("{}", full.join("."));
    }
    Ok(())
}

async fn cmd_tables_in(datalake: &Datalake, ns_str: &str) -> Result<()> {
    let catalog = datalake.get_catalog().await?;
    let ns = NamespaceIdent::from_strs(ns_str.split('.').map(|s| s.to_string()))
        .map_err(|e| BoxErr::from(e.to_string()))?;
    let tables = catalog
        .list_tables(&ns)
        .await
        .map_err(|e| BoxErr::from(e.to_string()))?;
    if tables.is_empty() {
        println!("(命名空间 {ns_str} 下无表)");
        return Ok(());
    }
    for t in tables {
        println!("{}", t.name);
    }
    Ok(())
}

async fn cmd_schema(ctx: &SessionContext, ident: &str) -> Result<()> {
    // LIMIT 1 探一下 schema（避免拉全表）
    let sql = format!("SELECT * FROM iceberg.{ident} LIMIT 1");
    let df = ctx.sql(&sql).await.map_err(|e| BoxErr::from(e.to_string()))?;
    let schema = df.schema();
    println!("{ident} 的列:");
    for f in schema.fields() {
        println!("  {} : {}", f.name(), f.data_type());
    }
    Ok(())
}

async fn cmd_count(ctx: &SessionContext, ident: &str) -> Result<()> {
    let sql = format!("SELECT COUNT(*) AS n FROM iceberg.{ident}");
    let df = ctx.sql(&sql).await.map_err(|e| BoxErr::from(e.to_string()))?;
    let batches: Vec<RecordBatch> = df.collect().await.map_err(|e| BoxErr::from(e.to_string()))?;
    let n = batches
        .first()
        .and_then(|b| b.column_by_name("n"))
        .map(|c| c.as_ref())
        .and_then(|a| a.as_any().downcast_ref::<arrow_array::Int64Array>())
        .map(|a| a.value(0))
        .unwrap_or(0);
    println!("{ident}: {n} 行");
    Ok(())
}

async fn cmd_drop(datalake: &Datalake, ident: &str) -> Result<()> {
    let (ns, table) = parse_ident(ident)?;
    let catalog = datalake.get_catalog().await?;
    let ti = TableIdent::new(ns, table);
    catalog
        .drop_table(&ti)
        .await
        .map_err(|e| BoxErr::from(e.to_string()))?;
    println!("已删除 {ident}");
    Ok(())
}

async fn cmd_upload(
    datalake: &Datalake,
    file: &str,
    ident: &str,
    fmt: FileFormat,
    overwrite: bool,
) -> Result<()> {
    let (ns, table) = parse_ident(ident)?;
    let ctx = datalake.get_ctx().await?;

    // 1. 读文件成 DataFrame
    let df = match fmt {
        FileFormat::Csv => ctx
            .read_csv(file, CsvReadOptions::new().has_header(true))
            .await
            .map_err(|e| BoxErr::from(format!("read_csv: {e}")))?,
        FileFormat::Parquet => ctx
            .read_parquet(file, ParquetReadOptions::default())
            .await
            .map_err(|e| BoxErr::from(format!("read_parquet: {e}")))?,
    };

    // 2. 派生 Iceberg schema
    let arrow_schema = df.schema().inner();
    let iceberg_schema = arrow_schema_to_schema_auto_assign_ids(arrow_schema)
        .map_err(|e| BoxErr::from(format!("iceberg schema: {e}")))?;

    // 3. 建表（overwrite 先 drop）
    let catalog = datalake.get_catalog().await?;
    let ti = TableIdent::new(ns.clone(), table.clone());
    if overwrite {
        let _ = catalog.drop_table(&ti).await; // best-effort
    }
    let creation = TableCreation::builder()
        .name(table.clone())
        .schema(iceberg_schema)
        .build();
    datalake
        .create_table_if_not_exist(&ns, creation)
        .await
        .map_err(|e| BoxErr::from(format!("create table: {e}")))?;

    // 4. fresh ctx（新表对旧 provider 不可见）+ INSERT
    let fresh = datalake.get_ctx().await?;
    let src = "__upload_src";
    fresh
        .register_table(src, df.into_view())
        .map_err(|e| BoxErr::from(format!("register: {e}")))?;
    let mut parts = vec!["iceberg".to_string()];
    parts.extend(ns.inner().iter().cloned());
    parts.push(table);
    let fqn = parts.join(".");
    let sql = format!("INSERT INTO {fqn} SELECT * FROM {src}");
    fresh
        .sql(&sql)
        .await
        .map_err(|e| BoxErr::from(format!("insert: {e}")))?
        .collect()
        .await
        .map_err(|e| BoxErr::from(format!("insert collect: {e}")))?;
    println!("上传完成 → {fqn}");
    Ok(())
}

#[derive(Clone, Copy)]
enum FileFormat {
    Csv,
    Parquet,
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_default();
    let rest: Vec<String> = args.collect();

    let datalake = Datalake::new();

    match cmd.as_str() {
        "query" => {
            let sql = rest.first().ok_or_else(|| BoxErr::from("query 需要 SQL"))?;
            cmd_query(&datalake.get_ctx().await?, sql).await
        }
        "sql-file" => {
            let path = rest.first().ok_or_else(|| BoxErr::from("sql-file 需要路径"))?;
            cmd_sql_file(&datalake.get_ctx().await?, path).await
        }
        "namespaces" => cmd_namespaces(&datalake).await,
        "tables" => cmd_tables(&datalake).await,
        "tables-in" => {
            let ns = rest
                .first()
                .ok_or_else(|| BoxErr::from("tables-in 需要命名空间"))?;
            cmd_tables_in(&datalake, ns).await
        }
        "schema" => {
            let ident = rest.first().ok_or_else(|| BoxErr::from("schema 需要 ident"))?;
            cmd_schema(&datalake.get_ctx().await?, ident).await
        }
        "count" => {
            let ident = rest.first().ok_or_else(|| BoxErr::from("count 需要 ident"))?;
            cmd_count(&datalake.get_ctx().await?, ident).await
        }
        "drop" => {
            let ident = rest.first().ok_or_else(|| BoxErr::from("drop 需要 ident"))?;
            cmd_drop(&datalake, ident).await
        }
        "upload" => {
            let file = rest
                .first()
                .ok_or_else(|| BoxErr::from("upload 需要文件路径"))?;
            let ident = rest
                .get(1)
                .ok_or_else(|| BoxErr::from("upload 需要 ident"))?;
            // 解析 flags
            let mut fmt = infer_format(file);
            let mut overwrite = false;
            for a in &rest[2..] {
                match a.as_str() {
                    "--csv" => fmt = FileFormat::Csv,
                    "--parquet" => fmt = FileFormat::Parquet,
                    "--overwrite" => overwrite = true,
                    other => return Err(BoxErr::from(format!("upload 未知参数: {other}"))),
                }
            }
            cmd_upload(&datalake, file, ident, fmt, overwrite).await
        }
        "" => usage(),
        other => {
            eprintln!("未知子命令: {other}");
            usage();
        }
    }
}

fn infer_format(path: &str) -> FileFormat {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".parquet") || lower.ends_with(".pq") {
        FileFormat::Parquet
    } else {
        FileFormat::Csv // 默认 csv
    }
}