"""Ingest human reference genome (GRCh37 or GRCh38) into the Iceberg data lake.

Downloads (or reads pre-downloaded) Ensembl genome data, parses it into
structured Iceberg tables, and optionally archives the raw files to OSS.

The FASTA is downloaded as per-chromosome files (more reliable than the single
large primary_assembly.fa.gz from rate-limited FTP connections), in batches of
5 with resume support (``curl -C -``).

Two tables are created in the ``reference`` namespace per assembly:

- ``reference.{prefix}_contigs`` — one row per chromosome / contig with
  length and MD5 checksum.
- ``reference.{prefix}_genes`` — parsed GTF gene annotation, one row per
  feature (gene, transcript, exon, CDS, …) with structured attribute
  columns (gene_id, gene_name, biotype, transcript_id, …).

Usage
-----
    # GRCh37 (Ensembl archive, release 87)
    python main.py --assembly grch37 --staging-dir ../../reference/grch37 --mode overwrite

    # GRCh38 (Ensembl release 116)
    python main.py --assembly grch38 --staging-dir ../../reference/grch38 --mode overwrite

    # Use pre-downloaded per-chromosome files
    python main.py --assembly grch38 \\
        --fasta-dir ../../reference/grch38 \\
        --gtf ../../reference/grch38/Homo_sapiens.GRCh38.116.gtf.gz \\
        --mode overwrite

    # Skip ingest, just archive raw files to OSS
    python main.py --assembly grch38 --staging-dir ../../reference/grch38 --archive-only
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import os
import re
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

import pyarrow as pa

from datalake import get_catalog

# ---------------------------------------------------------------------------
# Assembly configuration
# ---------------------------------------------------------------------------

CHROMOSOMES = [*map(str, range(1, 23)), "X", "Y", "MT"]
NAMESPACE = "reference"


@dataclass(frozen=True)
class AssemblyConfig:
    """All assembly-specific parameters for the ingest pipeline."""

    key: str               # grch37 / grch38 — used for table names + paths
    ensembl_name: str       # GRCh37 / GRCh38 — in filenames
    release_label: str      # human-readable release for docs
    ftp_base: str           # Ensembl FTP base URL for this assembly
    gtf_filename: str       # GTF .gz filename
    staging_dir: Path       # local download directory

    @property
    def fasta_glob(self) -> str:
        return f"Homo_sapiens.{self.ensembl_name}.dna.chromosome.*.fa.gz"

    @property
    def fasta_url_base(self) -> str:
        return f"{self.ftp_base}/fasta/homo_sapiens/dna"

    @property
    def gtf_url(self) -> str:
        return f"{self.ftp_base}/gtf/homo_sapiens/{self.gtf_filename}"

    @property
    def contig_table(self) -> str:
        return f"{self.key}_contigs"

    @property
    def gene_table(self) -> str:
        return f"{self.key}_genes"

    @property
    def archive_remote(self) -> str:
        return f"aliyun:autonomics-data/reference/{self.key}"

    def fasta_filename(self, chrom: str) -> str:
        return f"Homo_sapiens.{self.ensembl_name}.dna.chromosome.{chrom}.fa.gz"

    @staticmethod
    def for_assembly(assembly: str, staging_dir: Path) -> AssemblyConfig:
        if assembly == "grch37":
            return AssemblyConfig(
                key="grch37",
                ensembl_name="GRCh37",
                release_label="GRCh37.p13 / Ensembl r87",
                ftp_base="https://ftp.ensembl.org/pub/grch37/current",
                gtf_filename="Homo_sapiens.GRCh37.87.gtf.gz",
                staging_dir=staging_dir,
            )
        if assembly == "grch38":
            return AssemblyConfig(
                key="grch38",
                ensembl_name="GRCh38",
                release_label="GRCh38.p14 / Ensembl r116",
                ftp_base="https://ftp.ensembl.org/pub/release-116",
                gtf_filename="Homo_sapiens.GRCh38.116.gtf.gz",
                staging_dir=staging_dir,
            )
        raise ValueError(f"Unknown assembly: {assembly!r} (expected 'grch37' or 'grch38')")


# ---------------------------------------------------------------------------
# Download helpers
# ---------------------------------------------------------------------------


def download(url: str, dest: Path) -> Path:
    """Download *url* to *dest* if it doesn't already exist."""
    if dest.exists() and dest.stat().st_size > 0:
        print(f"  [skip] {dest.name} already exists ({dest.stat().st_size:,} bytes)")
        return dest
    print(f"  [download] {url}")
    print(f"  → {dest}")
    subprocess.run(
        ["curl", "-L", "--fail", "-C", "-", "-o", str(dest), url],
        check=True,
    )
    print(f"  done ({dest.stat().st_size:,} bytes)")
    return dest


def download_fasta(cfg: AssemblyConfig) -> list[Path]:
    """Download per-chromosome FASTA files (batched for FTP rate-limit friendliness).

    Returns a sorted list of .fa.gz paths — one per chromosome.
    """
    base = cfg.fasta_url_base
    batch_size = 5

    paths = []
    for i in range(0, len(CHROMOSOMES), batch_size):
        batch = CHROMOSOMES[i : i + batch_size]
        procs = []
        for chrom in batch:
            fname = cfg.fasta_filename(chrom)
            dest = cfg.staging_dir / fname
            paths.append(dest)
            if dest.exists() and dest.stat().st_size > 0:
                # Verify gzip integrity before skipping
                try:
                    with gzip.open(dest, "rb") as _fh:
                        _fh.read(1024)
                    continue
                except (gzip.BadGzipFile, EOFError):
                    pass  # truncated — re-download
            p = subprocess.Popen(
                ["curl", "-sL", "-C", "-", "-o", str(dest), f"{base}/{fname}"]
            )
            procs.append(p)
        for p in procs:
            p.wait()
        print(f"  batch {i // batch_size + 1}: chr{batch[0]}–{batch[-1]} done")

    return sorted(paths)


def ensure_files(cfg: AssemblyConfig) -> tuple[list[Path], Path]:
    """Ensure FASTA + GTF exist in staging_dir, downloading if necessary.

    Returns ``(fasta_paths, gtf_path)`` where *fasta_paths* is a list of
    per-chromosome .fa.gz files.
    """
    fasta_glob = sorted(cfg.staging_dir.glob(cfg.fasta_glob))
    gtf_path = cfg.staging_dir / cfg.gtf_filename

    if len(fasta_glob) < len(CHROMOSOMES) or not gtf_path.exists():
        print(f"=== Downloading {cfg.ensembl_name} data from Ensembl FTP ===")
        fasta_glob = download_fasta(cfg)
        download(cfg.gtf_url, gtf_path)

    return fasta_glob, gtf_path


# ---------------------------------------------------------------------------
# FASTA parsing → contig metadata
# ---------------------------------------------------------------------------


def iter_fasta(gz_path: Path):
    """Yield ``(header, sequence_lines)`` from a gzipped FASTA file.

    *sequence_lines* is a list of decoded line strings (without newlines).
    The caller is responsible for concatenation / checksum.
    """
    header: str | None = None
    seq_lines: list[str] = []
    with gzip.open(gz_path, "rt") as fh:
        for line in fh:
            line = line.rstrip("\n")
            if line.startswith(">"):
                if header is not None:
                    yield header, seq_lines
                header = line[1:]
                seq_lines = []
            else:
                seq_lines.append(line)
    if header is not None:
        yield header, seq_lines


def iter_fasta_multi(gz_paths: list[Path]):
    """Iterate over multiple per-chromosome FASTA files."""
    for path in gz_paths:
        yield from iter_fasta(path)


def parse_contig_name(header: str) -> str:
    """Extract the chromosome / contig name from a FASTA header.

    Ensembl headers look like: ``1 dna:chromosome chromosome:GRCh37:1:1:249250621:1``
    """
    return header.split()[0]


def build_contig_table(fasta_paths: list[Path]) -> pa.Table:
    """Parse FASTA file(s) to build an Arrow table of contig metadata.

    Columns: contig (string), length (int32), md5 (string).
    """
    contigs: list[str] = []
    lengths: list[int] = []
    md5s: list[str] = []

    for header, seq_lines in iter_fasta_multi(fasta_paths):
        name = parse_contig_name(header)
        # Concatenate sequence for length + MD5 (matches `samtools faidx` MD5)
        full_seq = "".join(seq_lines).upper()
        md5 = hashlib.md5(full_seq.encode("ascii")).hexdigest()

        contigs.append(name)
        lengths.append(len(full_seq))
        md5s.append(md5)
        print(f"  {name}: {len(full_seq):,} bp  md5={md5}")

    return pa.Table.from_arrays(
        [
            pa.array(contigs, type=pa.string()),
            pa.array(lengths, type=pa.int32()),
            pa.array(md5s, type=pa.string()),
        ],
        names=["contig", "length", "md5"],
    )


# ---------------------------------------------------------------------------
# GTF parsing → gene annotation table
# ---------------------------------------------------------------------------

# Attribute regex: key "value";
_ATTR_RE = re.compile(r'(\w+)\s+"([^"]*)"')


def parse_gtf_attributes(attr_str: str) -> dict[str, str]:
    """Parse the GTF attribute column into a dict."""
    return dict(_ATTR_RE.findall(attr_str))


_GTF_COL_NAMES = [
    "contig",
    "source",
    "feature",
    "start",
    "end",
    "score",
    "strand",
    "frame",
    "attributes",
]


def iter_gtf(gz_path: Path):
    """Yield parsed GTF rows as dicts (generator)."""
    with gzip.open(gz_path, "rt") as fh:
        for line in fh:
            if line.startswith("#"):
                continue
            fields = line.rstrip("\n").split("\t")
            if len(fields) < 9:
                continue
            row = dict(zip(_GTF_COL_NAMES, fields[:9]))
            attrs = parse_gtf_attributes(row["attributes"])
            row["_attrs"] = attrs
            yield row


def build_gene_table(gtf_path: Path) -> pa.Table:
    """Parse the GTF to build a structured Arrow table.

    Each row is one GTF feature (gene, transcript, exon, CDS, …) with the
    most common attributes promoted to typed columns.
    """
    arrays: dict[str, list] = {
        "contig": [],
        "source": [],
        "feature": [],
        "start": [],
        "end_pos": [],
        "strand": [],
        "frame": [],
        "gene_id": [],
        "gene_name": [],
        "gene_biotype": [],
        "transcript_id": [],
        "transcript_name": [],
        "transcript_biotype": [],
        "exon_id": [],
        "exon_number": [],
        "protein_id": [],
    }

    total = 0
    for row in iter_gtf(gtf_path):
        attrs = row["_attrs"]

        arrays["contig"].append(row["contig"])
        arrays["source"].append(row["source"])
        arrays["feature"].append(row["feature"])
        arrays["start"].append(int(row["start"]))
        arrays["end_pos"].append(int(row["end"]))
        arrays["strand"].append(row["strand"])
        # frame: ".", "0", "1", "2"
        frame_str = row["frame"]
        arrays["frame"].append(frame_str if frame_str != "." else None)
        arrays["gene_id"].append(attrs.get("gene_id"))
        arrays["gene_name"].append(attrs.get("gene_name"))
        arrays["gene_biotype"].append(attrs.get("gene_biotype"))
        arrays["transcript_id"].append(attrs.get("transcript_id"))
        arrays["transcript_name"].append(attrs.get("transcript_name"))
        arrays["transcript_biotype"].append(attrs.get("transcript_biotype"))
        arrays["exon_id"].append(attrs.get("exon_id"))
        exon_num = attrs.get("exon_number")
        arrays["exon_number"].append(int(exon_num) if exon_num else None)
        arrays["protein_id"].append(attrs.get("protein_id"))

        total += 1
        if total % 500_000 == 0:
            print(f"  parsed {total:,} features…")

    print(f"  Total: {total:,} features")

    schema = pa.schema(
        [
            pa.field("contig", pa.string()),
            pa.field("source", pa.string()),
            pa.field("feature", pa.string()),
            pa.field("start", pa.int32()),
            pa.field("end_pos", pa.int32()),
            pa.field("strand", pa.string()),
            pa.field("frame", pa.string()),
            pa.field("gene_id", pa.string()),
            pa.field("gene_name", pa.string()),
            pa.field("gene_biotype", pa.string()),
            pa.field("transcript_id", pa.string()),
            pa.field("transcript_name", pa.string()),
            pa.field("transcript_biotype", pa.string()),
            pa.field("exon_id", pa.string()),
            pa.field("exon_number", pa.int32()),
            pa.field("protein_id", pa.string()),
        ]
    )

    return pa.Table.from_arrays(
        [pa.array(arrays[col], type=schema.field(col).type) for col in schema.names],
        schema=schema,
    )


# ---------------------------------------------------------------------------
# Iceberg ingest
# ---------------------------------------------------------------------------


def _pa_to_iceberg_schema(pa_schema: pa.Schema):
    """Convert a PyArrow schema to a PyIceberg Schema."""
    from pyiceberg.schema import Schema
    from pyiceberg.types import (
        DoubleType,
        IntegerType,
        NestedField,
        StringType,
    )

    _type_map = {
        pa.string(): StringType(),
        pa.int32(): IntegerType(),
        pa.int64(): IntegerType(),
        pa.float64(): DoubleType(),
    }

    fields = []
    for i, field in enumerate(pa_schema):
        iceberg_type = _type_map.get(field.type, StringType())
        fields.append(NestedField(i + 1, field.name, iceberg_type, required=False))
    return Schema(*fields)


def create_or_load_table(catalog, full_table: str, pa_schema: pa.Schema):
    """Create or load an Iceberg table matching the Arrow schema."""
    iceberg_schema = _pa_to_iceberg_schema(pa_schema)
    if catalog.table_exists(full_table):
        existing = catalog.load_table(full_table)
        print(f"  Loading existing table: {full_table}")
        return existing
    print(f"  Creating table: {full_table}")
    return catalog.create_table(full_table, schema=iceberg_schema)


def sink_contigs(
    fasta_paths: list[Path], cfg: AssemblyConfig, namespace: str, mode: str
) -> None:
    """Parse FASTA → contig metadata → Iceberg table."""
    print("=== Parsing FASTA for contig metadata ===")
    t0 = time.time()
    arrow_tbl = build_contig_table(fasta_paths)
    print(f"  Parsed {arrow_tbl.num_rows} contigs in {time.time() - t0:.1f}s")

    print("=== Sinking contigs to Iceberg ===")
    catalog = get_catalog()
    print("  Catalog:", catalog.name)
    catalog.create_namespace_if_not_exists(namespace=namespace)

    full = f"{namespace}.{cfg.contig_table}"
    tbl = create_or_load_table(catalog, full, arrow_tbl.schema)
    if mode == "overwrite" and tbl.snapshots():
        tbl.overwrite(arrow_tbl)
    else:
        tbl.append(arrow_tbl)
    print(f"  Ingested {arrow_tbl.num_rows} contigs → {full}")


def sink_genes(
    gtf_path: Path, cfg: AssemblyConfig, namespace: str, mode: str
) -> None:
    """Parse GTF → gene annotation → Iceberg table."""
    print("=== Parsing GTF gene annotation ===")
    t0 = time.time()
    arrow_tbl = build_gene_table(gtf_path)
    print(f"  Built table: {arrow_tbl.num_rows:,} rows × {arrow_tbl.num_columns} cols in {time.time() - t0:.1f}s")

    print("=== Sinking genes to Iceberg ===")
    catalog = get_catalog()
    catalog.create_namespace_if_not_exists(namespace=namespace)

    full = f"{namespace}.{cfg.gene_table}"
    tbl = create_or_load_table(catalog, full, arrow_tbl.schema)
    if mode == "overwrite" and tbl.snapshots():
        tbl.overwrite(arrow_tbl)
    else:
        tbl.append(arrow_tbl)
    print(f"  Ingested {arrow_tbl.num_rows:,} features → {full}")


# ---------------------------------------------------------------------------
# OSS archival
# ---------------------------------------------------------------------------


def archive_to_oss(cfg: AssemblyConfig, files: list[Path]) -> None:
    """Upload raw files to aliyun OSS via rclone."""
    remote = cfg.archive_remote
    print(f"=== Archiving to {remote} ===")
    rclone = os.environ.get("RCLONE", "rclone")
    try:
        subprocess.run([rclone, "version"], capture_output=True, check=True)
    except (FileNotFoundError, subprocess.CalledProcessError):
        print("  [warn] rclone not found — skipping archive. "
              "Install rclone and configure the 'aliyun' remote.")
        return

    for f in files:
        dest = f"{remote}/{f.name}"
        print(f"  {f.name} → {dest}")
        subprocess.run(
            [rclone, "copyto", str(f), dest, "-P"],
            check=True,
        )
        print(f"  done: {f.name}")

    # Also write a README manifest
    manifest = cfg.staging_dir / "ARCHIVE_README.txt"
    manifest.write_text(
        f"{cfg.release_label} — archived {time.strftime('%Y-%m-%d')}\n"
        f"Source: {cfg.ftp_base}\n"
        f"Files:\n"
        + "".join(f"  - {f.name} ({f.stat().st_size:,} bytes)\n" for f in files)
    )
    subprocess.run(
        [rclone, "copyto", str(manifest), f"{remote}/ARCHIVE_README.txt"],
        check=True,
    )
    print(f"  manifest uploaded")


# ---------------------------------------------------------------------------
# Main CLI
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Ingest human reference genome (GRCh37/GRCh38) into Iceberg"
    )
    parser.add_argument(
        "--assembly",
        choices=["grch37", "grch38"],
        required=True,
        help="Genome assembly to ingest",
    )
    parser.add_argument(
        "--staging-dir",
        type=Path,
        default=None,
        help="Directory for downloaded files (default: ../../reference/{assembly})",
    )
    parser.add_argument(
        "--fasta-dir",
        type=Path,
        default=None,
        help="Directory with per-chromosome .fa.gz files (skip download)",
    )
    parser.add_argument(
        "--gtf",
        type=Path,
        default=None,
        help="Pre-downloaded GTF .gtf.gz path (skip download)",
    )
    parser.add_argument(
        "--fasta-glob",
        type=str,
        default=None,
        help="Glob pattern for per-chromosome FASTA files (auto-detected from assembly)",
    )
    parser.add_argument(
        "--namespace",
        default=NAMESPACE,
        help=f"Iceberg namespace (default: {NAMESPACE})",
    )
    parser.add_argument(
        "--mode",
        choices=["append", "overwrite"],
        default="overwrite",
        help="Write mode (default: overwrite)",
    )
    parser.add_argument(
        "--archive-only",
        action="store_true",
        help="Skip Iceberg ingest, only archive raw files to OSS",
    )
    parser.add_argument(
        "--skip-archive",
        action="store_true",
        help="Skip OSS archival",
    )
    parser.add_argument(
        "--skip-fasta",
        action="store_true",
        help="Skip FASTA/contig ingest (GTF only)",
    )
    args = parser.parse_args()

    # Build assembly config
    staging_dir = args.staging_dir or Path(f"../../reference/{args.assembly}")
    cfg = AssemblyConfig.for_assembly(args.assembly, staging_dir.resolve())

    fasta_glob_pattern = args.fasta_glob or cfg.fasta_glob

    # Resolve file paths
    fasta_search_dir = (args.fasta_dir or cfg.staging_dir).resolve()
    gtf_path = args.gtf

    if gtf_path:
        fasta_paths = sorted(fasta_search_dir.glob(fasta_glob_pattern))
        if not fasta_paths:
            print(f"No FASTA files matching {fasta_glob_pattern} in {fasta_search_dir}")
            sys.exit(1)
    else:
        fasta_paths, gtf_path = ensure_files(cfg)

    files_to_archive = [*fasta_paths, gtf_path]

    if args.archive_only:
        archive_to_oss(cfg, files_to_archive)
        return

    # Ingest to Iceberg
    if not args.skip_fasta:
        sink_contigs(fasta_paths, cfg, args.namespace, args.mode)
    sink_genes(gtf_path, cfg, args.namespace, args.mode)

    # Archive raw files
    if not args.skip_archive:
        archive_to_oss(cfg, files_to_archive)

    print(f"\n=== Done ({cfg.release_label}) ===")


if __name__ == "__main__":
    main()
