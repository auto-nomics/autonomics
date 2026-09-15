//! Per-format [`BioDriver`](super::core::BioDriver) implementations.
//!
//! Each module wires one oxbow scanner into the generic core by translating
//! "bytes → header/schema" and "bytes + batch size → batch iterator".

pub mod bam;
pub mod bcf;
pub mod bed;
pub mod bigbed;
pub mod bigwig;
pub mod cram;
pub mod fasta;
pub mod fastq;
pub mod gff;
pub mod gtf;
pub mod mtx;
pub mod sam;
pub mod vcf;

pub use bam::BamDriver;
pub use bcf::BcfDriver;
pub use bed::BedDriver;
pub use bigbed::BigBedDriver;
pub use bigwig::BigWigDriver;
pub use cram::CramDriver;
pub use fasta::FastaDriver;
pub use fastq::FastqDriver;
pub use gff::GffDriver;
pub use gtf::GtfDriver;
pub use mtx::MtxDriver;
pub use sam::SamDriver;
pub use vcf::VcfDriver;
