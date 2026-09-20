"""Build the ALS/CNS cell marker data package from the official CellMarker 2.0
Human spreadsheet (downloaded from the WilsonWukz/EasyCellMarker2 mirror, which
serves the official Oct 2024 snapshot).

The package contains three files:

- `markers.tsv`  — one row per (gene, marker_set, evidence) triple. Columns:
  gene_symbol, set_id, set_name, set_class, evidence, marker_source,
  tissue_class, tissue_type, pmid, journal, year, species. Suitable for direct
  ingestion into `enrichment_ora` after a long-to-wide reshape.
- `metadata.tsv` — set-level metadata: set_id, set_name, set_class,
  description, source_url, snapshot_date, gene_count, marker_source_filter.
- `source_manifest.json` — provenance: download URL, file SHA-256, commit
  pin and snapshot date.

Run:

    python3 fixtures/als_cns_cell_markers/build_markers.py \\
        --xlsx /tmp/Cell_marker_Human.xlsx \\
        --out fixtures/als_cns_cell_markers/staging \\
        --snapshot-date 2024-10-11
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
from pathlib import Path

import openpyxl


SETS = {
    "Astrocyte": ("ALS_CNS_ASTROCYTE", "Astrocyte", "Glial"),
    "Microglial cell": ("ALS_CNS_MICROGLIA", "Microglia", "Glial"),
    "Oligodendrocyte": ("ALS_CNS_OLIGODENDROCYTE", "Oligodendrocyte", "Glial"),
    "Neuron": ("ALS_CNS_NEURON", "Neuron", "Neuronal"),
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--xlsx", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--snapshot-date", default="2024-10-11")
    parser.add_argument(
        "--source-url",
        default="https://raw.githubusercontent.com/WilsonWukz/EasyCellMarker2/master/inst/extdata/Cell_marker_Human.xlsx",
    )
    parser.add_argument(
        "--commit",
        default="c870a9ff6a8b48390a2fe6fe8054b88738a821ad",
    )
    return parser.parse_args()


def collect_rows(xlsx_path: Path):
    workbook = openpyxl.load_workbook(xlsx_path, read_only=True, data_only=True)
    sheet = workbook["human"]
    header = [c.value for c in next(sheet.iter_rows(max_row=1))]
    idx = {name: i for i, name in enumerate(header)}
    rows_by_set: dict[str, list[dict]] = {set_id: [] for _, (set_id, *_rest) in SETS.items()}
    seen_keys: set[tuple[str, str, str, str]] = set()
    for row in sheet.iter_rows(min_row=2, values_only=True):
        cell_name = row[idx["cell_name"]]
        if cell_name not in SETS:
            continue
        if row[idx["species"]] != "Human":
            continue
        tissue_class = row[idx["tissue_class"]]
        if tissue_class not in {"Brain", "Spinal cord"}:
            continue
        marker_source = row[idx["marker_source"]]
        if marker_source not in {"Experiment", "Review"}:
            continue
        symbol = row[idx["Symbol"]]
        if not symbol or not isinstance(symbol, str):
            continue
        symbol = symbol.strip()
        if not symbol:
            continue
        evidence = "canonical" if marker_source == "Experiment" else "supporting"
        pmid = row[idx["PMID"]] or ""
        journal = row[idx["journal"]] or ""
        year = row[idx["year"]] or ""
        set_id, set_name, set_class = SETS[cell_name]
        key = (symbol, set_id, str(pmid), evidence)
        if key in seen_keys:
            continue
        seen_keys.add(key)
        rows_by_set[set_id].append(
            {
                "gene_symbol": symbol,
                "set_id": set_id,
                "set_name": set_name,
                "set_class": set_class,
                "evidence": evidence,
                "marker_source": marker_source,
                "tissue_class": tissue_class,
                "tissue_type": row[idx["tissue_type"]] or "",
                "pmid": str(pmid),
                "journal": journal,
                "year": str(year),
                "species": "Human",
            }
        )
    return rows_by_set


def write_markers_tsv(path: Path, rows: dict[str, list[dict]]):
    columns = [
        "gene_symbol", "set_id", "set_name", "set_class",
        "evidence", "marker_source", "tissue_class", "tissue_type",
        "pmid", "journal", "year", "species",
    ]
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=columns, delimiter="\t")
        writer.writeheader()
        for set_id in sorted(rows):
            for row in sorted(rows[set_id], key=lambda r: (r["gene_symbol"], r["pmid"], r["evidence"])):
                writer.writerow(row)


def write_metadata_tsv(path: Path, rows: dict[str, list[dict]], snapshot_date: str, source_url: str):
    columns = [
        "set_id", "set_name", "set_class", "description",
        "source_url", "snapshot_date", "gene_count", "marker_source_filter",
    ]
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=columns, delimiter="\t")
        writer.writeheader()
        for cell_name, (set_id, set_name, set_class) in SETS.items():
            gene_symbols = {r["gene_symbol"] for r in rows[set_id]}
            writer.writerow(
                {
                    "set_id": set_id,
                    "set_name": set_name,
                    "set_class": set_class,
                    "description": (
                        f"{set_name} markers curated from CellMarker 2.0 Human "
                        f"snapshot {snapshot_date} for ALS/CNS tissue_class Brain/Spinal cord"
                    ),
                    "source_url": source_url,
                    "snapshot_date": snapshot_date,
                    "gene_count": len(gene_symbols),
                    "marker_source_filter": "Experiment|Review",
                }
            )


def write_source_manifest(path: Path, xlsx_path: Path, snapshot_date: str, source_url: str, commit: str, rows: dict[str, list[dict]]):
    sha = hashlib.sha256(xlsx_path.read_bytes()).hexdigest()
    manifest = {
        "id": "als_cns_cell_markers",
        "version": "v1.0.0",
        "kind": "cell_marker_set",
        "metadata": {
            "snapshot_date": snapshot_date,
            "source_url": source_url,
            "source_repository": "https://github.com/WilsonWukz/EasyCellMarker2",
            "source_revision": commit,
            "license": "CC BY 4.0 (CellMarker 2.0)",
            "cell_names": list(SETS.keys()),
            "tissue_filter": "Brain|Spinal cord",
            "marker_source_filter": "Experiment|Review",
            "species": "Human",
        },
        "files": {
            "markers.tsv": {
                "rows": sum(len(v) for v in rows.values()),
                "sets": sorted(rows.keys()),
            },
            "metadata.tsv": {
                "rows": len(SETS),
            },
        },
        "source_payload": {
            "filename": xlsx_path.name,
            "sha256": sha,
            "size": xlsx_path.stat().st_size,
        },
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(manifest, indent=2, sort_keys=True))


def main():
    args = parse_args()
    rows = collect_rows(args.xlsx)
    out = args.out
    out.mkdir(parents=True, exist_ok=True)
    write_markers_tsv(out / "markers.tsv", rows)
    write_metadata_tsv(out / "metadata.tsv", rows, args.snapshot_date, args.source_url)
    write_source_manifest(
        out / "source_manifest.json",
        args.xlsx,
        args.snapshot_date,
        args.source_url,
        args.commit,
        rows,
    )
    totals = {set_id: len({r["gene_symbol"] for r in recs}) for set_id, recs in rows.items()}
    print(json.dumps({"genes_per_set": totals, "rows": {k: len(v) for k, v in rows.items()}}, indent=2))


if __name__ == "__main__":
    main()
