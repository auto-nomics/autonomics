#!/usr/bin/env python3
"""Extract common SNPs from 3 GWAS VCFs, compute Z-scores, and export a
test dataset for CPASSOC cross-validation.

Extracts SNPs from chr1:155-156Mb across 3 GWAS traits, finds common SNPs,
and writes:
  - bio_crates/cpassoc/tests/real_gwas_zscores.csv  (M×K Z-score matrix)
  - Prints sample sizes for each trait
"""
import subprocess
import csv
import os

TRAITS = [
    "ebi-a-GCST90018877",
    "ebi-a-GCST90018864",
    "ebi-a-GCST90018890",
]
BASE = "/mnt/disk3/test"
REGION = "1:155000000-156000000"
OUTPUT = os.path.join(
    os.path.dirname(__file__), "real_gwas_zscores.csv"
)

def extract_trait(trait):
    """Extract rsID, ES, SE, AF, SS from VCF. Returns dict rsID -> (z, ss)."""
    vcf = f"{BASE}/{trait}/{trait}.vcf.gz"
    result = subprocess.run(
        ["tabix", vcf, REGION],
        capture_output=True, text=True
    )
    snps = {}
    sample_size = None
    for line in result.stdout.strip().split("\n"):
        if not line:
            continue
        cols = line.split("\t")
        rsid = cols[2]
        if rsid == ".":
            continue
        fmt_keys = cols[8].split(":")
        fmt_vals = cols[9].split(":")
        d = dict(zip(fmt_keys, fmt_vals))
        try:
            es = float(d.get("ES", "0"))
            se = float(d.get("SE", "0"))
            lp = float(d.get("LP", "0"))
            ss = int(float(d.get("SS", "0")))
        except (ValueError, KeyError):
            continue
        if se == 0:
            continue
        # Z-score: sign from ES, magnitude from LP
        z = es / se
        # Use LP to get a more stable |Z|: |Z| = sqrt(2*LN(10)*LP) ≈ sqrt(4.6*LP)
        import math
        abs_z = math.sqrt(2 * math.log(10) * lp) if lp > 0 else abs(z)
        z = abs_z if z >= 0 else -abs_z
        snps[rsid] = (z, ss)
        if sample_size is None:
            sample_size = ss
    return snps, sample_size

# Extract from each trait
all_snps = []
trait_data = []
sample_sizes = []
for trait in TRAITS:
    snps, ss = extract_trait(trait)
    print(f"{trait}: {len(snps)} SNPs, sample_size={ss}")
    all_snps.append(set(snps.keys()))
    trait_data.append(snps)
    sample_sizes.append(ss)

# Find common SNPs
common = all_snps[0] & all_snps[1] & all_snps[2]
print(f"Common SNPs: {len(common)}")

# Build matrix: take first 200 common SNPs for a manageable test
common_sorted = sorted(common)[:200]
print(f"Using {len(common_sorted)} SNPs")

# Write CSV
with open(OUTPUT, "w", newline="") as f:
    writer = csv.writer(f)
    writer.writerow(["rsid"] + [f"trait_{i+1}" for i in range(len(TRAITS))])
    for rsid in common_sorted:
        row = [rsid]
        for i in range(len(TRAITS)):
            z = trait_data[i][rsid][0]
            row.append(f"{z:.6f}")
        writer.writerow(row)

# Print sample sizes for use in tests
print(f"\nSample sizes: {sample_sizes}")
print(f"Output: {OUTPUT}")
