#!/bin/bash
#
# Compute per-chromosome LD matrices for specified ancestries from 1000G VCFs
#
# Directory layout (organized by population → chromosome):
#   $OUT_ROOT/<pop>/
#   ├── samples.txt          sample list for this ancestry (FID IID)
#   ├── chr<N>/              QC'd PLINK bfile for each chromosome
#   │   ├── 1000G.<pop>.chr<N>.qc.bed
#   │   ├── 1000G.<pop>.chr<N>.qc.bim
#   │   ├── 1000G.<pop>.chr<N>.qc.fam
#   │   └── 1000G.<pop>.chr<N>.qc.frq  ← MAF (used for hvec)
#   └── ld/
#       └── 1000G.<pop>.chr<N>.ld.vcor.zst   ← sparse LD matrix
#
# Usage:
#   ./ld_matrix.sh                        # run all ancestries × chr1-22
#   ./ld_matrix.sh EUR                    # run EUR only
#   ./ld_matrix.sh EUR EAS                # run EUR and EAS
#   POPS="EAS AFR" CHRS="21 22" ./ld_matrix.sh   # specify via env vars
#
set -euo pipefail

# ──────────────── Configuration ────────────────
VCF_ROOT="/mnt/disk2/dataset/1000g_genotype_data"
OUT_ROOT="/mnt/disk2/dataset/1000g_plink/pearson_r"
PANEL="${VCF_ROOT}/integrated_call_samples_v3.20130502.ALL.panel"

# Default: all 5 super-populations + chr1-22; overridable via env vars / positional args
ALL_POPS="EUR EAS AFR SAS AMR"
POPS="${POPS:-${*:-$ALL_POPS}}"
# CHRS="${CHRS:-$(seq 1 22)}"
CHRS="22"

# LD computation parameters (consistent with MiXeR reference)
MAF_MIN=0.01       # minimum minor allele frequency (statistical power floor at 1000G N=503)
GENO_MAX=0.05      # maximum SNP missingness rate
LD_WINDOW_KB=10000 # LD computation window (10Mb, LD decay distance)
LD_R2_MIN=0.01     # storage r² threshold
THREADS="${THREADS:-8}"
MEM_MB="${MEM_MB:-12000}"

# ──────────────── Utility functions ────────────────
log() { echo "[$(date '+%H:%M:%S')] $*" >&2; }

# Generate the sample list for a given ancestry (idempotent)
ensure_samples() {
  local pop="$1" out_dir="$2"
  local sf="$out_dir/samples.txt"
  mkdir -p "$out_dir"
  if [[ ! -s "$sf" ]]; then
    # When PLINK2 reads the VCF, FID=0, IID=sample ID
    awk -F'\t' -v pop="$pop" '$3==pop{print "0\t"$1}' "$PANEL" >"$sf"
    log "$pop: generated sample list with $(wc -l <"$sf") individuals"
  fi
}

# Per-chromosome pipeline: VCF → QC bfile + freq → LD matrix
process_chr() {
  local pop="$1" chr="$2"
  local pop_dir="$OUT_ROOT/$pop"
  local chr_dir="$pop_dir/chr${chr}"
  local ld_dir="$pop_dir/ld"
  local prefix="$chr_dir/1000G.${pop}.chr${chr}.qc"
  local vcf="$VCF_ROOT/ALL.chr${chr}.phase3_shapeit2_mvncall_integrated_v5a.20130502.genotypes.vcf.gz"

  # chrX has a different filename (v1b instead of v5a)
  if [[ "$chr" == "X" ]]; then
    vcf="$VCF_ROOT/ALL.chrX.phase3_shapeit2_mvncall_integrated_v1b.20130502.genotypes.vcf.gz"
  fi

  mkdir -p "$chr_dir" "$ld_dir"
  local ld_out="$ld_dir/1000G.${pop}.chr${chr}.ld.vcor.zst"

  # Idempotent: skip the whole chromosome if LD output already exists
  if [[ -s "$ld_out" ]]; then
    log "$pop chr$chr: LD output already exists, skipping"
    return 0
  fi

  if [[ ! -f "$vcf" ]]; then
    log "$pop chr$chr: ⚠ VCF missing ($vcf), skipping"
    return 0
  fi

  # Step 1: VCF → population subset → QC → bfile (idempotent)
  if [[ ! -f "${prefix}.bed" ]]; then
    log "$pop chr$chr: VCF → QC bfile"
    plink2 --vcf "$vcf" \
      --keep "$pop_dir/samples.txt" \
      --max-alleles 2 \
      --maf "$MAF_MIN" --geno "$GENO_MAX" \
      --threads "$THREADS" --memory "$MEM_MB" \
      --make-bed --out "$prefix" \
      2>&1 | grep -iE "variant|sample|remaining|error" | grep -vE "^--vcf|--r2" || true
  fi

  # Step 2: compute MAF (for hvec = 2·maf·(1-maf), idempotent)
  if [[ ! -f "${prefix}.frq" ]]; then
    log "$pop chr$chr: computing MAF"
    plink2 --bfile "$prefix" --freq --threads "$THREADS" --out "$prefix" 2>&1 | tail -2 || true
  fi

  # Step 3: compute LD matrix (idempotent: output goes to the ld/ directory)
  log "$pop chr$chr: computing LD matrix (r²≥$LD_R2_MIN, window ${LD_WINDOW_KB}kb)"
  plink2 --bfile "$prefix" \
    --r-unphased zs \
    --ld-window 999999 \
    --ld-window-kb "$LD_WINDOW_KB" \
    --ld-window-r2 "$LD_R2_MIN" \
    --threads "$THREADS" --memory "$MEM_MB" \
    --out "$ld_dir/1000G.${pop}.chr${chr}.ld" \
    2>&1 | grep -iE "Running|filters|done|error" || true

  log "$pop chr$chr: done → $(du -h "$ld_out" 2>/dev/null | cut -f1)"
}

# ──────────────── Main pipeline ────────────────
log "Start: ancestries=[$POPS] chromosomes=[$CHRS]"

for pop in $POPS; do
  pop_dir="$OUT_ROOT/$pop"
  ensure_samples "$pop" "$pop_dir"
  for chr in $CHRS; do
    process_chr "$pop" "$chr"
  done
  log "$pop: all chromosomes done, total size $(du -sh "$pop_dir/ld" 2>/dev/null | cut -f1)"
done

log "All done"
