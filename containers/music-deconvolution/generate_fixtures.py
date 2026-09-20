#!/usr/bin/env python3
"""Generate a deterministic MuSiC smoke fixture.

The reference has four donors, two cell types, and ten cells per donor/type.
Bulk samples are constructed from the same expected type profiles with known,
varying proportions. Counts are deterministic rounded expected values so
package checksums and numerical baselines remain stable.
"""

from __future__ import annotations

import csv
from pathlib import Path


def write_matrix(path: Path, rows: list[list[object]]) -> None:
    with path.open("w", newline="") as handle:
        writer = csv.writer(handle, delimiter="\t", lineterminator="\n")
        writer.writerows(rows)


def main() -> None:
    root = Path(__file__).parent / "fixtures"
    root.mkdir(parents=True, exist_ok=True)
    genes = [f"GENE{index:03d}" for index in range(120)]
    # The first 45 genes are neuron-enriched, the next 45 astrocyte-enriched,
    # and the remainder provide a common low-expression backbone.
    neuron_profile = []
    astrocyte_profile = []
    for index in range(120):
        if index < 45:
            neuron_profile.append(28.0 + (index % 7) * 1.5)
            astrocyte_profile.append(3.0 + (index % 5) * 0.5)
        elif index < 90:
            neuron_profile.append(3.0 + (index % 5) * 0.5)
            astrocyte_profile.append(28.0 + (index % 7) * 1.5)
        else:
            neuron_profile.append(8.0 + (index % 4) * 0.5)
            astrocyte_profile.append(9.0 + (index % 4) * 0.5)

    donors = [f"donor_{index}" for index in range(1, 5)]
    cell_columns = []
    expression_rows = [[gene] for gene in genes]
    for donor_index, donor in enumerate(donors):
        donor_neuron_scale = 0.85 + donor_index * 0.09
        donor_astrocyte_scale = 1.16 - donor_index * 0.08
        for cell_type, profile, donor_scale in [
            ("neuron", neuron_profile, donor_neuron_scale),
            ("astrocyte", astrocyte_profile, donor_astrocyte_scale),
        ]:
            for cell_index in range(10):
                cell_id = f"{donor}_{cell_type[0]}{cell_index:02d}"
                cell_columns.append(
                    {
                        "cell_id": cell_id,
                        "subject_id": donor,
                        "cell_type": cell_type,
                    }
                )
                values = []
                for gene_index, expected in enumerate(profile):
                    cell_effect = 0.92 + ((donor_index * 13 + cell_index * 7 + gene_index) % 9) / 45.0
                    lambda_value = max(0.05, expected * donor_scale * cell_effect)
                    values.append(round(lambda_value))
                for gene_index in range(len(genes)):
                    expression_rows[gene_index].append(values[gene_index])

    single_cell_header = ["gene_id", *[cell["cell_id"] for cell in cell_columns]]
    write_matrix(root / "single_cell_counts.tsv", [single_cell_header, *expression_rows])
    with (root / "cell_metadata.tsv").open("w", newline="") as handle:
        fieldnames = ["cell_id", "subject_id", "cell_type"]
        writer = csv.DictWriter(handle, fieldnames=fieldnames, delimiter="\t", lineterminator="\n")
        writer.writeheader()
        writer.writerows(cell_columns)

    mixtures = [
        ("bulk_70n", 0.70),
        ("bulk_60n", 0.60),
        ("bulk_50n", 0.50),
        ("bulk_40n", 0.40),
        ("bulk_30n", 0.30),
        ("bulk_20n", 0.20),
    ]
    bulk_rows = [["gene_id", *[sample for sample, _ in mixtures]]]
    for gene_index, gene in enumerate(genes):
        values = []
        for sample_index, (_, neuron_fraction) in enumerate(mixtures):
            sample_scale = 0.95 + sample_index * 0.02
            expected = (
                neuron_fraction * neuron_profile[gene_index]
                + (1.0 - neuron_fraction) * astrocyte_profile[gene_index]
            ) * sample_scale
            values.append(f"{expected:.6f}")
        bulk_rows.append([gene, *values])
    write_matrix(root / "bulk_expression.tsv", bulk_rows)

    marker_rows = [["gene_id"], *[[gene] for gene in genes[:20]]]
    write_matrix(root / "markers.tsv", marker_rows)
    write_matrix(
        root / "cell_sizes.tsv",
        [["cell_type", "cell_size"], ["neuron", "1.0"], ["astrocyte", "1.0"]],
    )

    print(f"wrote {len(cell_columns)} reference cells and {len(mixtures)} bulk samples")


if __name__ == "__main__":
    main()
