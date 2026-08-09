//! Core data types for the LaTeX writing system.
//!
//! This crate is intentionally free of I/O and external API dependencies.
//! It defines the structured document model ([`Document`], [`Section`],
//! [`Block`], [`Inline`], [`Claim`]) that sits between the agent editing
//! layer and the LaTeX serialisation / compilation pipeline.
//!
//! Keeping the types in a leaf crate prevents circular dependencies:
//! `writing-base` can depend on `writing-types` without pulling in storage
//! code, and future node bundles can import the types independently.

pub mod block;
pub mod claim;
pub mod document;
pub mod edit;
pub mod figure;
pub mod inline;
pub mod section;

pub use block::{
    Block, BlockComment, BlockId, BlockMeta, CodeBlock, EquationBlock, HorizontalRule, ListBlock,
    ListItem, ListMarker, PageBreak, ParagraphBlock, QuoteBlock, RawLatexBlock,
};
pub use claim::{
    Claim, ClaimType, Confidence, EvidenceLink, EvidenceTarget, EvidenceType, TextSpan,
};
pub use document::{
    Document, DocumentAuthor, DocumentClass, DocumentMetadata, MacroDef, Preamble, TemplateSpec,
};
pub use edit::{
    CitationPosition, EditOp, EditScript, MetadataChanges, Outline, OutlineItem, PreambleChanges,
};
pub use figure::{
    ColumnAlign, FigureBlock, FigureSource, Placement, Size, SubfigureSpec, TableBlock, TableCell,
    TableFormat, TableSource,
};
pub use inline::{CitationCluster, CitationStyle, CiteKey, Inline, RefKind, TextFormat};
pub use section::{Section, SectionLevel, SectionStatus};

/// Generate a new block ID (UUID v4).
pub fn new_block_id() -> BlockId {
    uuid_v4_string()
}

fn uuid_v4_string() -> String {
    // Simple UUID v4 generation without pulling in the uuid crate.
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    // Mix in thread id for extra uniqueness within a process.
    let tid = std::thread::current().id();
    let tid_hash = format!("{tid:?}")
        .bytes()
        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));

    let seed = nanos.wrapping_add(tid_hash as u128);
    let bytes = seed.to_le_bytes();

    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    )
}
