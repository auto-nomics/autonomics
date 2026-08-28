#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: build_eqtlgen_besd.sh

Convert the eQTLGen full cis-eQTL release into SMR BESD format.

Environment:
  EQTLGEN_INPUT       Original eQTLGen .txt.gz (required unless USE_PARQUET=1)
  EQTLGEN_PARQUET     Optional Parquet copy with the same columns
  USE_PARQUET=1       Read EQTLGEN_PARQUET instead of decompressing the raw file
  REF_ROOT            1000G EUR PLINK panel prefix root
  WORK                Output/work directory
  THREADS             plink2/SMR thread count (default 4)
  DUCKDB_MEMORY_GB    DuckDB memory limit (default 6)
  SMR_MEMORY          Podman memory limit (default 16g)
  FORCE=1             Replace a previously generated WORK directory
  SKIP_FREQ=1         Reuse existing .afreq files
  SKIP_BUILD=1        Skip matrix/BESD generation
  SKIP_MAPPING=1      Reuse matrix and annotation files in an existing WORK
  RESUME=1            Permit running with an existing WORK directory

The clean package is written to:
  $WORK/package/eqtlgen_hg19.besd
  $WORK/package/eqtlgen_hg19.esi
  $WORK/package/eqtlgen_hg19.epi
EOF
}

[[ "${1:-}" == "-h" || "${1:-}" == "--help" ]] && {
  usage
  exit 0
}

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing required command: $1" >&2
    exit 1
  }
}

need plink2
need duckdb
need podman
need awk
need sha256sum

input=${EQTLGEN_INPUT:-/mnt/data/eqtlgen/2019-12-11-cis-eQTLsFDR-ProbeLevel-CohortInfoRemoved-BonferroniAdded.txt.gz}
parquet=${EQTLGEN_PARQUET:-/mnt/data/eqtlgen/cis-eqtlgen.parquet}
use_parquet=${USE_PARQUET:-0}
ref_root=${REF_ROOT:-"/home/wjx/.autonomics/panels/plink.ref.1000g_eur.binary@sha256:80597da4137e90c3c312c9fa37a4dae9d29f18b7a8b12576ae502941bb1a558d"}
work=${WORK:-/mnt/data/eqtlgen/smr_build}
threads=${THREADS:-4}
duckdb_memory_gb=${DUCKDB_MEMORY_GB:-6}
smr_memory=${SMR_MEMORY:-16g}
force=${FORCE:-0}
skip_freq=${SKIP_FREQ:-0}
skip_build=${SKIP_BUILD:-0}
skip_mapping=${SKIP_MAPPING:-0}
resume=${RESUME:-0}
image=${SMR_IMAGE:-192.168.10.24:30500/atc/smr:1.4.2}

if [[ "$use_parquet" == 1 ]]; then
  [[ -f "$parquet" ]] || {
    echo "USE_PARQUET=1 but missing input: $parquet" >&2
    exit 1
  }
  source_relation="read_parquet('$parquet')"
else
  [[ -f "$input" ]] || {
    echo "missing eQTLGen input: $input" >&2
    exit 1
  }
  source_relation="read_csv('$input', header=true, delim='\t', columns={ \
    'Pvalue':'DOUBLE', 'SNP':'VARCHAR', 'SNPChr':'INT', 'SNPPos':'BIGINT', \
    'AssessedAllele':'VARCHAR', 'OtherAllele':'VARCHAR', 'Zscore':'DOUBLE', \
    'Gene':'VARCHAR', 'GeneSymbol':'VARCHAR', 'GeneChr':'INT', \
    'GenePos':'BIGINT', 'NrCohorts':'SMALLINT', 'NrSamples':'INT', \
    'FDR':'DOUBLE', 'BonferroniP':'DOUBLE' \
  })"
fi

if [[ -e "$work" && "$force" == 1 ]]; then
  rm -rf -- "$work"
fi

if [[ -e "$work" && "$resume" != 1 ]] ; then
  cat >&2 <<EOF
WORK directory already exists: $work
Set FORCE=1 to replace it, or choose a new WORK path.
EOF
  exit 1
fi

for chrom in $(seq 1 22); do
  prefix="$ref_root/1000G.EUR.QC.$chrom"
  for extension in bed bim fam; do
    [[ -f "$prefix.$extension" ]] || {
      echo "missing reference file: $prefix.$extension" >&2
      exit 1
    }
  done
done

mkdir -p "$work"/{afreq,logs,build,package,validation}

if [[ "$skip_freq" != 1 ]]; then
  echo "Generating 1000G EUR allele frequencies..."
  for chrom in $(seq 1 22); do
    prefix="$ref_root/1000G.EUR.QC.$chrom"
    plink2 \
      --bfile "$prefix" \
      --freq cols=chrom,pos,ref,alt1,alt1freq,nobs \
      --threads "$threads" \
      --out "$work/afreq/chr$chrom" \
      >"$work/logs/plink2.chr$chrom.log" 2>&1
  done
else
  count=$(find "$work/afreq" -maxdepth 1 -name 'chr*.afreq' | wc -l)
  [[ "$count" -eq 22 ]] || {
    echo "SKIP_FREQ=1 requires 22 .afreq files; found $count" >&2
    exit 1
  }
fi

if [[ "$skip_build" != 1 ]]; then
  if [[ "$skip_mapping" == 1 ]]; then
    for suffix in matrix.txt.gz esi epi; do
      [[ -s "$work/eqtlgen.$suffix" ]] || {
        echo "SKIP_MAPPING=1 but missing: $work/eqtlgen.$suffix" >&2
        exit 1
      }
    done
  fi

  echo "Mapping eQTLGen variants and writing SMR text annotations..."
  if [[ "$skip_mapping" != 1 ]]; then
  duckdb "$work/eqtlgen_mapping.duckdb" <<SQL
  PRAGMA memory_limit='${duckdb_memory_gb}GB';
  PRAGMA threads=$threads;
  PRAGMA temp_directory='$work/.duckdb-temp';

  CREATE VIEW eqtlgen AS
    SELECT * FROM $source_relation;

  CREATE TABLE source_stats AS
  SELECT
    count(*) AS source_rows,
    count(*) FILTER (WHERE Zscore = 0) AS zero_z_rows,
    count(DISTINCT SNP) AS source_snps,
    count(DISTINCT Gene) AS source_genes
  FROM eqtlgen;

  CREATE TABLE mapped_all AS
  WITH reference_variants AS MATERIALIZED (
    SELECT
      "#CHROM"::INT AS chrom,
      POS::INT AS pos,
      ID AS ref_snp,
      REF,
      ALT1,
      ALT1_FREQ
    FROM read_csv_auto('$work/afreq/chr*.afreq', header=true, delim='\t')
    QUALIFY row_number() OVER (
      PARTITION BY "#CHROM", POS, REF, ALT1
      ORDER BY ID
    ) = 1
  )
  SELECT
    r.ref_snp,
    s.SNP AS source_snp,
    s.SNPChr,
    s.SNPPos,
    s.AssessedAllele,
    s.OtherAllele,
    CASE
      WHEN s.AssessedAllele = r.ALT1 THEN r.ALT1_FREQ
      WHEN s.AssessedAllele = r.REF THEN 1.0 - r.ALT1_FREQ
    END AS a1_freq,
    s.Zscore,
    s.Pvalue,
    s.FDR,
    s.Gene,
    s.GeneSymbol,
    s.GeneChr,
    s.GenePos,
    row_number() OVER (
      PARTITION BY s.Gene, r.ref_snp
      ORDER BY (s.SNP <> r.ref_snp), s.Pvalue, s.SNP
    ) AS alias_rank
  FROM eqtlgen s
  JOIN reference_variants r
    ON s.SNPChr = r.chrom
   AND s.SNPPos = r.pos
   AND (
         (s.AssessedAllele = r.ALT1 AND s.OtherAllele = r.REF)
      OR (s.AssessedAllele = r.REF AND s.OtherAllele = r.ALT1)
   )
  WHERE s.Zscore <> 0;

  COPY (
    SELECT
      ref_snp AS SNP,
      Gene,
      Zscore AS beta,
      Zscore AS "t-stat",
      Pvalue AS "p-value",
      FDR
    FROM mapped_all
    WHERE alias_rank = 1
    ORDER BY Gene, SNPChr, SNPPos, ref_snp
  ) TO '$work/eqtlgen.matrix.txt.gz'
  (FORMAT CSV, DELIMITER '\t', HEADER, COMPRESSION gzip);

  COPY (
    SELECT
      SNPChr,
      ref_snp,
      0,
      SNPPos,
      AssessedAllele,
      OtherAllele,
      any_value(a1_freq)
    FROM mapped_all
    WHERE alias_rank = 1
    GROUP BY SNPChr, ref_snp, SNPPos, AssessedAllele, OtherAllele
    ORDER BY SNPChr, SNPPos, ref_snp
  ) TO '$work/eqtlgen.esi'
  (FORMAT CSV, DELIMITER '\t', HEADER false);

  COPY (
    SELECT DISTINCT
      GeneChr,
      Gene,
      0,
      GenePos,
      GeneSymbol,
      '+'
    FROM mapped_all
    WHERE alias_rank = 1
    ORDER BY GeneChr, GenePos, Gene
  ) TO '$work/eqtlgen.epi'
  (FORMAT CSV, DELIMITER '\t', HEADER false);

  CREATE TABLE mapping_stats AS
  SELECT
    (SELECT source_rows FROM source_stats) AS source_rows,
    (SELECT zero_z_rows FROM source_stats) AS zero_z_rows,
    (SELECT source_snps FROM source_stats) AS source_snps,
    (SELECT source_genes FROM source_stats) AS source_genes,
    count(*) FILTER (WHERE alias_rank = 1) AS retained_rows,
    count(*) FILTER (WHERE alias_rank > 1) AS alias_duplicate_rows,
    count(DISTINCT ref_snp) AS retained_snps,
    count(DISTINCT Gene) AS retained_genes
  FROM mapped_all;

  COPY mapping_stats TO '$work/mapping_stats.tsv'
  (FORMAT CSV, DELIMITER '\t', HEADER true);
SQL
  fi

  echo "Generating BESD with official SMR..."
  podman run --rm --tls-verify=false \
    -v "$work":/work \
    --memory "$smr_memory" \
    --memory-swap "$smr_memory" \
    --cpus "$threads" \
    --entrypoint smr \
    "$image" \
    --eqtl-summary /work/eqtlgen.matrix.txt.gz \
    --matrix-eqtl-format \
    --make-besd \
    --thread-num "$threads" \
    --out /work/build/eqtlgen_hg19 \
    >"$work/logs/make_besd.log" 2>&1

  echo "Completing .esi and .epi annotations..."
  podman run --rm --tls-verify=false \
    -v "$work":/work \
    --memory "$smr_memory" \
    --memory-swap "$smr_memory" \
    --cpus "$threads" \
    --entrypoint smr \
    "$image" \
    --beqtl-summary /work/build/eqtlgen_hg19 \
    --update-esi /work/eqtlgen.esi \
    --update-epi /work/eqtlgen.epi \
    >"$work/logs/update_annotations.log" 2>&1

  install -m 0644 "$work/build/eqtlgen_hg19.besd" \
    "$work/package/eqtlgen_hg19.besd"
  install -m 0644 "$work/build/eqtlgen_hg19.esi" \
    "$work/package/eqtlgen_hg19.esi"
  install -m 0644 "$work/build/eqtlgen_hg19.epi" \
    "$work/package/eqtlgen_hg19.epi"
fi

[[ -s "$work/package/eqtlgen_hg19.besd" ]] || {
  echo "missing BESD output: $work/package/eqtlgen_hg19.besd" >&2
  exit 1
}

esi_bad=$(awk -F '\t' '
  NF != 7 || $1 == "NA" || $2 == "NA" || $4 == "NA" || $5 == "NA" ||
  $6 == "NA" || $7 == "NA" { bad++ }
  END { print bad + 0 }
' "$work/package/eqtlgen_hg19.esi")
epi_bad=$(awk -F '\t' 'NF != 6 { bad++ } END { print bad + 0 }' \
  "$work/package/eqtlgen_hg19.epi")

[[ "$esi_bad" -eq 0 && "$epi_bad" -eq 0 ]] || {
  echo "invalid annotation rows: esi=$esi_bad epi=$epi_bad" >&2
  exit 1
}

echo "Querying one full probe to validate BESD readability..."
podman run --rm --tls-verify=false \
  -v "$work":/work \
  --memory "$smr_memory" \
  --memory-swap "$smr_memory" \
  --cpus "$threads" \
  --entrypoint smr \
  "$image" \
  --beqtl-summary /work/package/eqtlgen_hg19 \
  --query 1 \
  --probe ENSG00000172322 \
  --out /work/validation/clec12a \
  >"$work/logs/validation_query.log" 2>&1

[[ -s "$work/validation/clec12a.txt" ]] || {
  echo "SMR validation query produced no rows" >&2
  exit 1
}

(
  cd "$work/package"
  sha256sum eqtlgen_hg19.besd eqtlgen_hg19.esi eqtlgen_hg19.epi \
    >SHA256SUMS
)

echo "Conversion completed."
echo "Package: $work/package"
cat "$work/mapping_stats.tsv" 2>/dev/null || true
echo "Validation rows: $(wc -l <"$work/validation/clec12a.txt")"
