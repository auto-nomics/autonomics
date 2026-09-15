//! Single extension trait adding JSON and bioinformatics file readers to
//! DataFusion's [`SessionContext`].
//!
//! Every `read_<format>` helper is a thin wrapper over the generic
//! [`read_bio`](crate::datasource::read_bio), specialized on the matching
//! [`BioDriver`](crate::datasource::drivers). This mirrors DataFusion's own
//! `read_csv` / `read_parquet` surface.
//!
//! In addition to the generic whole-file readers, [`read_vcf_region`] performs
//! an indexed random-access query against a BGZF-compressed VCF, fetching only
//! the BGZF blocks overlapping the requested region via the oxbow async scanner
//! and a `.tbi`/`.csi` index.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arrow::array::RecordBatch;
use datafusion::catalog::MemTable;
use datafusion::error::DataFusionError;
use datafusion::execution::context::DataFilePaths;
use datafusion::prelude::{DataFrame, SessionContext};
use futures::{StreamExt, TryStreamExt};
use oxbow::async_scanner::AsyncScanner as _;
use oxbow::variant::VcfScanner;
use oxbow::{CoordSystem, Region as OxRegion, Select}; // bring trait methods into scope

use crate::datasource::drivers::{
    BamDriver, BcfDriver, BedDriver, BigBedDriver, BigWigDriver, CramDriver, FastaDriver,
    FastqDriver, GffDriver, GtfDriver, MtxDriver, SamDriver, VcfDriver,
};
use crate::datasource::{BioReadOptions, read_bio, read_json};
use datafusion::common::Result;

/// Extension trait that adds typed readers (`read_vcf`, `read_bam`, …) to a
/// [`SessionContext`], mirroring DataFusion's own `read_csv` / `read_parquet`.
pub trait DataFusionReadExt {
    /// Read VCF file(s) as a queryable [`DataFrame`].
    fn read_vcf<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send;

    /// Read BCF file(s).
    fn read_bcf<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send;

    /// Read FASTA file(s).
    fn read_fasta<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send;

    /// Read FASTQ file(s).
    fn read_fastq<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send;

    /// Read a MatrixMarket coordinate file as a long-table [`DataFrame`].
    fn read_mtx<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send;

    /// Read BED file(s).
    fn read_bed<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send;

    /// Read GTF file(s).
    fn read_gtf<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send;

    /// Read GFF file(s).
    fn read_gff<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send;

    /// Read SAM file(s).
    fn read_sam<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send;

    /// Read BAM file(s).
    fn read_bam<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send;

    /// Read CRAM file(s).
    fn read_cram<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send;

    /// Read BigWig file(s).
    fn read_bigwig<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send;

    /// Read BigBed file(s).
    fn read_bigbed<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send;

    /// Read a JSON array or newline-delimited JSON file(s).
    ///
    /// This has a distinct name from DataFusion's inherent
    /// `SessionContext::read_json` so importing this extension trait does not
    /// make plain JSON calls ambiguous.
    fn read_bio_json<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send;

    /// Indexed random-access read of a BGZF-compressed VCF.
    ///
    /// Loads the VCF header (small, sync), then streams only the BGZF blocks
    /// that overlap `region` using [`oxbow::async_scanner`] and a `.tbi`/`.csi`
    /// index. The index path is resolved by appending `.tbi` (preferred) or
    /// `.csi` to the VCF file path. Currently requires a local filesystem
    /// path (the async reader needs `AsyncRead + AsyncSeek` over the raw
    /// BGZF bytes).
    fn read_vcf_region<P: AsRef<Path>>(
        &self,
        table_path: P,
        region: &str,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send;
}

impl DataFusionReadExt for SessionContext {
    fn read_vcf<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send {
        read_bio::<VcfDriver, _>(self, table_paths, options)
    }
    fn read_bcf<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send {
        read_bio::<BcfDriver, _>(self, table_paths, options)
    }
    fn read_fasta<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send {
        read_bio::<FastaDriver, _>(self, table_paths, options)
    }
    fn read_fastq<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send {
        read_bio::<FastqDriver, _>(self, table_paths, options)
    }
    fn read_mtx<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send {
        read_bio::<MtxDriver, _>(self, table_paths, options)
    }
    fn read_bed<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send {
        read_bio::<BedDriver, _>(self, table_paths, options)
    }
    fn read_gtf<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send {
        read_bio::<GtfDriver, _>(self, table_paths, options)
    }
    fn read_gff<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send {
        read_bio::<GffDriver, _>(self, table_paths, options)
    }
    fn read_sam<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send {
        read_bio::<SamDriver, _>(self, table_paths, options)
    }
    fn read_bam<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send {
        read_bio::<BamDriver, _>(self, table_paths, options)
    }
    fn read_cram<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send {
        read_bio::<CramDriver, _>(self, table_paths, options)
    }
    fn read_bigwig<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send {
        read_bio::<BigWigDriver, _>(self, table_paths, options)
    }
    fn read_bigbed<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send {
        read_bio::<BigBedDriver, _>(self, table_paths, options)
    }
    fn read_bio_json<P: DataFilePaths + Send>(
        &self,
        table_paths: P,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send {
        read_json(self, table_paths, options)
    }
    fn read_vcf_region<P: AsRef<Path>>(
        &self,
        table_path: P,
        region: &str,
        options: BioReadOptions,
    ) -> impl std::future::Future<Output = Result<DataFrame>> + Send {
        let ctx = self.clone();
        let path: PathBuf = table_path.as_ref().to_path_buf();
        let region = region.to_string();
        let options = options.clone();
        async move { read_vcf_region_impl(&ctx, &path, &region, &options).await }
    }
}

/// Per-call unique table name so multiple `read_vcf_region` invocations don't
/// overwrite each other in the session catalog.
static REGION_TABLE_COUNTER: AtomicU64 = AtomicU64::new(0);

async fn read_vcf_region_impl(
    ctx: &SessionContext,
    path: &Path,
    region_str: &str,
    options: &BioReadOptions,
) -> Result<DataFrame> {
    // Parse the region string (UCSC `chr1:1000-2000` or bracket notation).
    let region: OxRegion = region_str
        .parse()
        .map_err(|e: oxbow::OxbowError| DataFusionError::External(Box::new(e)))?;

    // Read the VCF header synchronously to build the scanner (the header is
    // small and lives at the start of the BGZF stream). The file is assumed
    // BGZF-compressed — region queries only make sense against indexed BGZF.
    let header_file = std::fs::File::open(path)?;
    let bgzf_reader = noodles::bgzf::io::Reader::new(header_file);
    let mut vcf_reader = noodles::vcf::io::Reader::new(bgzf_reader);
    let header = vcf_reader
        .read_header()
        .map_err(|e| DataFusionError::External(Box::new(e)))?;
    let scanner = VcfScanner::new(
        header,
        Select::All,
        Select::All,
        Select::All,
        None,
        Select::All,
        None,
        CoordSystem::OneClosed,
    )
    .map_err(|e| DataFusionError::External(Box::new(e)))?;

    // Resolve the index path (try `.tbi` first, then `.csi`).
    let tbi_path = append_suffix(path, ".tbi");
    let csi_path = append_suffix(path, ".csi");

    // Open a seekable async handle on the raw BGZF bytes (the async scanner
    // seeks by BGZF virtual position).
    let vcf_file = tokio::fs::File::open(path).await?;

    let batch_size = options.batch_size().unwrap_or(8192);

    let batches: Vec<RecordBatch> = if tbi_path.exists() {
        let index = noodles::tabix::io::Reader::new(std::fs::File::open(&tbi_path)?)
            .read_index()
            .map_err(|e| DataFusionError::External(Box::new(e)))?;
        let stream = scanner
            .scan_async_query(
                vcf_file,
                region.clone(),
                &index,
                None,
                Some(batch_size),
                None,
            )
            .map_err(|e| DataFusionError::External(Box::new(e)))?;
        stream
            .map(|r| r.map_err(|e| DataFusionError::External(Box::new(e))))
            .try_collect()
            .await?
    } else if csi_path.exists() {
        let index = noodles::csi::io::Reader::new(std::fs::File::open(&csi_path)?)
            .read_index()
            .map_err(|e| DataFusionError::External(Box::new(e)))?;
        let stream = scanner
            .scan_async_query(
                vcf_file,
                region.clone(),
                &index,
                None,
                Some(batch_size),
                None,
            )
            .map_err(|e| DataFusionError::External(Box::new(e)))?;
        stream
            .map(|r| r.map_err(|e| DataFusionError::External(Box::new(e))))
            .try_collect()
            .await?
    } else {
        return Err(DataFusionError::Plan(format!(
            "no .tbi or .csi index found next to {}",
            path.display()
        )));
    };

    let schema = batches[0].schema();
    let table_name = format!(
        "vcf_region_{}",
        REGION_TABLE_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let provider = MemTable::try_new(Arc::clone(&schema), vec![batches])?;
    ctx.register_table(&table_name, Arc::new(provider))?;
    ctx.table(&table_name).await
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut p = path.to_path_buf();
    p.as_mut_os_string().push(suffix);
    p
}
