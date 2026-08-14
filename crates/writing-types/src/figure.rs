//! Figure and table blocks.

use serde::{Deserialize, Serialize};

use crate::block::BlockMeta;
use crate::inline::Inline;

// ---------------------------------------------------------------------------
// Placement & Size
// ---------------------------------------------------------------------------

/// LaTeX float placement specifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Placement {
    /// `[h]` — here.
    Here,
    /// `[t]` — top of page.
    Top,
    /// `[b]` — bottom of page.
    Bottom,
    /// `[p]` — float page.
    Page,
    /// `[H]` — exactly here (requires `float` package).
    ForceHere,
    /// Default (no specifier).
    Default,
}

impl Placement {
    pub fn specifier(&self) -> &'static str {
        match self {
            Self::Here => "h",
            Self::Top => "t",
            Self::Bottom => "b",
            Self::Page => "p",
            Self::ForceHere => "H",
            Self::Default => "",
        }
    }
}

impl Default for Placement {
    fn default() -> Self {
        Self::Default
    }
}

/// A dimension with unit (for `\includegraphics[width=...]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Size {
    pub value: f64,
    pub unit: String, // "textwidth", "linewidth", "cm", "mm", "in"
}

impl Size {
    /// `0.8\textwidth`
    pub fn text_width(fraction: f64) -> Self {
        Self {
            value: fraction,
            unit: "textwidth".into(),
        }
    }

    /// Render as a LaTeX dimension string.
    pub fn to_latex(&self) -> String {
        format!("{}\\{}", trim_float(self.value), self.unit)
    }
}

fn trim_float(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v}")
    }
}

// ---------------------------------------------------------------------------
// Figure
// ---------------------------------------------------------------------------

/// A figure block (`\begin{figure}...\end{figure}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FigureBlock {
    pub meta: BlockMeta,
    /// Caption (may contain inline formatting and citations).
    #[serde(default)]
    pub caption: Vec<Inline>,
    /// Float placement.
    #[serde(default)]
    pub placement: Placement,
    /// Image source.
    pub source: FigureSource,
    /// Width specifier for `\includegraphics`.
    #[serde(default)]
    pub width: Option<Size>,
    /// Subfigures — when non-empty, this is a `\begin{subfigure}` layout.
    #[serde(default)]
    pub subfigures: Vec<SubfigureSpec>,
}

/// Where the figure image comes from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FigureSource {
    /// A file path accessible to the LaTeX compiler.
    FilePath { path: String },
    /// Reference to a DAG node artifact (resolved before compilation).
    DagArtifact { dag_id: String, node_id: String },
    /// Inline TikZ code.
    Tikz { code: String },
}

/// A subfigure within a figure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubfigureSpec {
    pub source: FigureSource,
    #[serde(default)]
    pub caption: Vec<Inline>,
    #[serde(default)]
    pub width: Option<Size>,
}

// ---------------------------------------------------------------------------
// Table
// ---------------------------------------------------------------------------

/// A table block (`\begin{table}...\end{table}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableBlock {
    pub meta: BlockMeta,
    /// Caption.
    #[serde(default)]
    pub caption: Vec<Inline>,
    /// Float placement.
    #[serde(default)]
    pub placement: Placement,
    /// Data source.
    pub source: TableSource,
}

/// Where the table data comes from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TableSource {
    /// Explicit cell data.
    Cells {
        header: Vec<String>,
        #[serde(default)]
        rows: Vec<Vec<TableCell>>,
        #[serde(default)]
        alignment: Vec<ColumnAlign>,
    },
    /// DAG DataFrame reference (resolved before compilation).
    DagOutput {
        dag_id: String,
        node_id: String,
        #[serde(default)]
        port: Option<String>,
        #[serde(default)]
        columns: Option<Vec<String>>,
        #[serde(default)]
        max_rows: Option<usize>,
        format: TableFormat,
    },
}

/// Column alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColumnAlign {
    Left,
    Center,
    Right,
}

impl ColumnAlign {
    pub fn latex_char(&self) -> char {
        match self {
            Self::Left => 'l',
            Self::Center => 'c',
            Self::Right => 'r',
        }
    }
}

/// Table rendering format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TableFormat {
    /// `\begin{tabular}` with `\hline`.
    Plain,
    /// `booktabs` style with `\toprule` / `\midrule` / `\bottomrule`.
    Booktabs,
    /// `\begin{longtable}` for multi-page tables.
    Longtable,
}

impl Default for TableFormat {
    fn default() -> Self {
        Self::Booktabs
    }
}

/// A single table cell.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableCell {
    pub content: String,
    /// Whether the cell content is raw LaTeX (vs. plain text to be escaped).
    #[serde(default)]
    pub raw_latex: bool,
}

impl TableCell {
    pub fn plain(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            raw_latex: false,
        }
    }

    pub fn raw(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            raw_latex: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_specifiers() {
        assert_eq!(Placement::Here.specifier(), "h");
        assert_eq!(Placement::ForceHere.specifier(), "H");
        assert_eq!(Placement::Default.specifier(), "");
    }

    #[test]
    fn size_to_latex() {
        let s = Size::text_width(0.8);
        assert_eq!(s.to_latex(), r"0.8\textwidth");

        let s2 = Size {
            value: 5.0,
            unit: "cm".into(),
        };
        assert_eq!(s2.to_latex(), r"5\cm");
    }

    #[test]
    fn figure_source_filepath() {
        let f = FigureSource::FilePath {
            path: "figures/plot.pdf".into(),
        };
        let json = serde_json::to_string(&f).unwrap();
        let back: FigureSource = serde_json::from_str(&json).unwrap();
        match back {
            FigureSource::FilePath { path } => {
                assert_eq!(path, "figures/plot.pdf");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn table_cells_round_trip() {
        let t = TableSource::Cells {
            header: vec!["Method".into(), "Beta".into(), "SE".into()],
            rows: vec![vec![
                TableCell::plain("IVW"),
                TableCell::raw("0.45^{***}"),
                TableCell::plain("0.03"),
            ]],
            alignment: vec![ColumnAlign::Left, ColumnAlign::Center, ColumnAlign::Center],
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: TableSource = serde_json::from_str(&json).unwrap();
        match back {
            TableSource::Cells { header, rows, .. } => {
                assert_eq!(header, vec!["Method", "Beta", "SE"]);
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].len(), 3);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn column_align_chars() {
        assert_eq!(ColumnAlign::Left.latex_char(), 'l');
        assert_eq!(ColumnAlign::Center.latex_char(), 'c');
        assert_eq!(ColumnAlign::Right.latex_char(), 'r');
    }
}
