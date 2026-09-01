//! DataFusion integration for JSON and the bioinformatics file formats oxbow
//! supports.
//!
//! The integration is split into:
//! - [`core`]: a generic `FileFormat` / `FileSource` / `FileOpener` stack driven
//!   by the [`BioDriver`](core::BioDriver) trait, plus the shared
//!   [`BioReadOptions`] / [`read_bio`] entry point.
//! - [`drivers`]: one small [`BioDriver`](core::BioDriver) implementation per
//!   format (VCF, BCF, FASTA, FASTQ, BED, GTF, GFF, SAM, BAM, CRAM, BigWig,
//!   BigBed).
//! - [`json`]: JSON array and NDJSON reading through DataFusion's native JSON
//!   reader.
//!
//! Public readers (`read_vcf`, `read_bcf`, …) live in [`crate::ext`].

pub mod core;
pub mod drivers;
pub mod json;

pub use core::{
    BioBatchStream, BioDriver, BioFormat, BioFormatFactory, BioInput, BioOptions, BioReadOptions,
    BioSource, read_bio,
};
pub use json::read_json;
