//! # biofusion
//!
//! Apache Arrow DataFusion integration for JSON and bioinformatics file formats, built
//! on top of [`oxbow`].
//!
//! Each oxbow-supported format (VCF, BCF, FASTA, FASTQ, BED, GTF, GFF, SAM,
//! BAM, CRAM, BigWig, BigBed) is exposed as a typed reader on
//! [`SessionContext`] via [`DataFusionReadExt`](ext::DataFusionReadExt):
//!
//! ```no_run
//! # use biofusion::ext::DataFusionReadExt;
//! # use biofusion::datasource::BioReadOptions;
//! # use datafusion::prelude::SessionContext;
//! # async fn run() -> datafusion::common::Result<()> {
//! let ctx = SessionContext::new();
//! let df = ctx.read_vcf("sample.vcf", BioReadOptions::default()).await?;
//! df.show().await?;
//! # Ok(())
//! # }
//! ```
//!
//! Under the hood every format shares one generic
//! `FileFormat` / `FileSource` / `FileOpener` stack (see [`datasource::core`]);
//! each format only contributes a small driver. Only the read path is
//! implemented; writes return `NotImplemented`.
//!
//! JSON arrays and newline-delimited JSON can also be loaded as DataFrames:
//!
//! ```no_run
//! # use biofusion::ext::DataFusionReadExt;
//! # use biofusion::datasource::BioReadOptions;
//! # use datafusion::prelude::SessionContext;
//! # async fn run() -> datafusion::common::Result<()> {
//! let ctx = SessionContext::new();
//! let df = ctx.read_bio_json("records.json", BioReadOptions::default()).await?;
//! # let _ = df;
//! # Ok(())
//! # }
//! ```

pub mod datasource;
pub mod ext;
