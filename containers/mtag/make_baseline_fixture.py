#!/usr/bin/env python3
"""Build a deterministic two-trait MTAG fixture from LDSC baseline inputs."""

from __future__ import annotations

import argparse
import csv
import gzip
import math
import sys
from pathlib import Path


def open_text(path: str):
    if path.endswith(".gz"):
        return gzip.open(path, "rt")
    return open(path, "rt")


def read_ldsc(path: str):
    variants = {}
    with open_text(path) as handle:
        reader = csv.DictReader(handle, delimiter="\t")
        required = {"SNP", "A1", "A2", "N", "Z"}
        if not required.issubset(reader.fieldnames or []):
            raise ValueError(f"{path} lacks LDSC columns: {sorted(required)}")
        for row in reader:
            try:
                variants[row["SNP"]] = (
                    row["A1"].upper(),
                    row["A2"].upper(),
                    float(row["N"]),
                    float(row["Z"]),
                )
            except (KeyError, TypeError, ValueError):
                continue
    return variants


def chrom_scores(panel_dir: Path):
    for chrom in range(1, 23):
        path = panel_dir / f"{chrom}.l2.ldscore.gz"
        if not path.exists():
            raise FileNotFoundError(path)
        with gzip.open(path, "rt") as handle:
            for row in csv.DictReader(handle, delimiter="\t"):
                yield row


def write_row(handle, row, values):
    a1, a2, sample_size, zscore = values
    handle.write(
        "\t".join(
            [
                row["SNP"],
                a1,
                a2,
                row["MAF"],
                row["CHR"],
                row["BP"],
                f"{sample_size:g}",
                f"{zscore:.6g}",
                f"{math.erfc(abs(zscore) / math.sqrt(2.0)):.6g}",
            ]
        )
        + "\n"
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--panel-dir", required=True)
    parser.add_argument("--trait1", required=True)
    parser.add_argument("--trait2", required=True)
    parser.add_argument("--output1", required=True)
    parser.add_argument("--output2", required=True)
    parser.add_argument("--max-variants", type=int, default=200_000)
    args = parser.parse_args()

    trait1 = read_ldsc(args.trait1)
    trait2 = read_ldsc(args.trait2)
    output1 = Path(args.output1)
    output2 = Path(args.output2)
    output1.parent.mkdir(parents=True, exist_ok=True)
    output2.parent.mkdir(parents=True, exist_ok=True)

    written = 0
    header = "snpid\ta1\ta2\tfreq\tchr\tbpos\tn\tz\tp\n"
    with output1.open("w") as out1, output2.open("w") as out2:
        out1.write(header)
        out2.write(header)
        for row in chrom_scores(Path(args.panel_dir)):
            if written >= args.max_variants:
                break
            values1 = trait1.get(row["SNP"])
            values2 = trait2.get(row["SNP"])
            if values1 is None or values2 is None:
                continue
            if values1[:2] != values2[:2]:
                continue
            a1, a2 = values1[:2]
            if a1 in ("A", "T") and a2 in ("A", "T"):
                continue
            if a1 in ("C", "G") and a2 in ("C", "G"):
                continue
            try:
                maf = float(row["MAF"])
            except (TypeError, ValueError):
                continue
            if not 0.01 < maf < 0.99:
                continue
            write_row(out1, row, values1)
            write_row(out2, row, values2)
            written += 1

    if written != args.max_variants:
        raise RuntimeError(
            f"only found {written} aligned variants; expected {args.max_variants}"
        )
    print(f"wrote {written} aligned variants to {output1} and {output2}")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"error: {error}", file=sys.stderr)
        raise
