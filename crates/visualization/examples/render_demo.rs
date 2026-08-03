//! Render a sample scatter+line plot to a PNG, kept on disk for inspection.
//!
//! Run (with the r45 conda env active):
//!   R_HOME=.../envs/r45/lib/R PATH=.../envs/r45/bin:$PATH \
//!     cargo run -p visualization --example render_demo

use std::sync::Arc;

use arrow::array::{Float64Array, Int32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use visualization::render::render_png;

#[tokio::main]
async fn main() {
    // A small parabola: y = x^2 over x = 0..50.
    let xs: Vec<i32> = (0..50).collect();
    let ys: Vec<f64> = xs.iter().map(|x| (*x as f64).powi(2)).collect();

    let schema = Arc::new(Schema::new(vec![
        Field::new("x", DataType::Int32, false),
        Field::new("y", DataType::Float64, false),
    ]));
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int32Array::from(xs)),
            Arc::new(Float64Array::from(ys)),
        ],
    )
    .expect("build batch");

    let out = std::path::PathBuf::from("/tmp/viz_demo.png");
    let r_code = r#"
        p <- ggplot(df, aes(x = x, y = y)) +
             geom_point(color = "steelblue", size = 2) +
             geom_line(color = "firebrick", linewidth = 0.8) +
             labs(title = "y = x^2  (DataFusion → R via Arrow IPC)",
                  x = "x", y = "y = x^2") +
             theme_minimal()
    "#;

    let diag = render_png(
        std::slice::from_ref(&batch),
        r_code,
        &out,
        Some(7.0),
        Some(5.0),
        Some(120.0),
    )
    .await
    .expect("render should succeed");

    let bytes = std::fs::metadata(&out).expect("output exists").len();
    println!("rendered {bytes} bytes to {}", out.display());
    if !diag.warnings.is_empty() {
        println!("R warnings ({}):", diag.warnings.len());
        for w in &diag.warnings {
            println!("  • {w}");
        }
    }
    if !diag.messages.is_empty() {
        println!("R messages ({}):", diag.messages.len());
        for m in &diag.messages {
            println!("  • {m}");
        }
    }
}
