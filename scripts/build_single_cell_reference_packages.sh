#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
download_root=${DOWNLOAD_ROOT:-/mnt/data/single_cell_reference_downloads}
output_root=${OUTPUT_ROOT:-/mnt/data/single_cell_catalog_packages}
config=${CATALOG_CONFIG:-$HOME/.autonomics/vfs.toml}
publish=${PUBLISH_PACKAGES:-0}
catalog=(cargo run -q --manifest-path "$root/Cargo.toml" -p data-catalog --bin autonomics-catalog --)

mkdir -p "$download_root" "$output_root"

fetch() {
  local url=$1
  local destination=$2
  mkdir -p "$(dirname "$destination")"
  curl --fail --location --retry 3 --silent --show-error "$url" -o "$destination"
}

write_cpdb_readme() {
  cat >"$1" <<'EOF'
# CellPhoneDB v5.0.0 ligand-receptor package

Source: https://github.com/ventolab/cellphonedb-data/tree/v5.0.0
License: MIT (as recorded by OmniPath resource metadata)

The `source/` directory contains the official v5.0.0 inputs. The normalized
Parquet tables preserve the official rows. `lr_pairs.parquet` follows CellPhoneDB
v5 ordering: partner A is the ligand and partner B is the receptor. Complexes
are expanded to their member HGNC gene symbols; `ligand_is_complex` and
`receptor_is_complex` retain that information.

Citation: Efremova et al., Nature Protocols 2020; Garcia-Alonso et al.,
Nature 2022; and the CellPhoneDB v5 release notes.
EOF
}

write_ramilowski_readme() {
  cat >"$1" <<'EOF'
# Ramilowski 2015 ligand-receptor package

Source: Ramilowski et al., Nature Communications 6:7866 (2015), obtained from
the OmniPath `ligrecextra` dataset with `resources=Ramilowski2015`.
License: CC BY 4.0.

`interactions.parquet` preserves the API result. `lr_pairs.parquet` uses
OmniPath/Ramilowski orientation: `source_genesymbol` is the ligand and
`target_genesymbol` is the receptor.
EOF
}

write_hgnc_readme() {
  cat >"$1" <<'EOF'
# HGNC approved symbol and alias dictionary

Source: HGNC complete set from the official public-download Google Storage
bucket. HGNC imposes no access or use restrictions; cite "HUGO Gene Nomenclature
Committee at the University of Cambridge" and https://www.genenames.org/.

`hgnc_complete_set.parquet` preserves the official complete set. `alias_dictionary.parquet`
explodes current alias symbols and previous symbols into one row per alias.
EOF
}

write_celltypist_readme() {
  cat >"$1" <<'EOF'
# CellTypist Pan-immune models

Source: the official CellTypist model service at
https://celltypist.cog.sanger.ac.uk/models/.

This package includes `Immune_All_Low.pkl` and `Immune_All_High.pkl` from
Pan_Immune_CellTypist v2. It is intended for explicit File input to
`h5ad_celltypist_annotate`. The models describe immune populations from 20
tissues and 18 studies (Domínguez Conde et al., Science 2022).
EOF
}

build_cellphonedb() {
  local stage=$download_root/cellphonedb-v5.0.0
  local source=$stage/source
  local base=https://raw.githubusercontent.com/ventolab/cellphonedb-data/v5.0.0/data
  rm -rf "$stage"
  mkdir -p "$source"
  for file in \
    interaction_input.csv \
    protein_input.csv \
    gene_input.csv \
    complex_input.csv \
    transcription_factor_input.csv
  do
    fetch "$base/$file" "$source/$file"
  done
  fetch \
    https://raw.githubusercontent.com/ventolab/cellphonedb-data/v5.0.0/cellphonedb.zip \
    "$stage/source/cellphonedb.zip"
  fetch \
    https://raw.githubusercontent.com/ventolab/cellphonedb-data/v5.0.0/README.md \
    "$stage/source/README.md"
  write_cpdb_readme "$stage/README.md"

  python - "$stage" <<'PY'
from pathlib import Path
import sys

import pandas as pd

stage = Path(sys.argv[1])
source = stage / "source"

frames = {}
for name in (
    "interaction_input", "protein_input", "gene_input", "complex_input",
    "transcription_factor_input",
):
    frames[name] = pd.read_csv(source / f"{name}.csv", dtype=str, keep_default_na=False)
    frames[name].to_parquet(
        stage / f"{name}.parquet", index=False, engine="pyarrow", compression="zstd"
    )

genes = frames["gene_input"]
gene_by_uniprot = {}
for row in genes.itertuples(index=False):
    symbol = row.hgnc_symbol or row.gene_name or row.uniprot
    gene_by_uniprot.setdefault(row.uniprot, set()).add(symbol)

complexes = frames["complex_input"]
complex_genes = {}
for row in complexes.itertuples(index=False):
    symbols = set()
    for uniprot in (row.uniprot_1, row.uniprot_2, row.uniprot_3, row.uniprot_4, row.uniprot_5):
        if uniprot:
            symbols.update(gene_by_uniprot.get(uniprot, {uniprot}))
    complex_genes[row.complex_name] = sorted(symbols)

def partner_genes(partner):
    if partner in complex_genes:
        return complex_genes[partner], True
    if partner in gene_by_uniprot:
        return sorted(gene_by_uniprot[partner]), False
    return [partner], False

records = []
for index, row in enumerate(frames["interaction_input"].itertuples(index=False), start=1):
    ligand_genes, ligand_complex = partner_genes(row.partner_a)
    receptor_genes, receptor_complex = partner_genes(row.partner_b)
    for ligand_gene in ligand_genes:
        for receptor_gene in receptor_genes:
            records.append({
                "interaction_id": row.id_cp_interaction or f"CPDBv5:{index}",
                "ligand": ligand_gene,
                "receptor": receptor_gene,
                "ligand_partner": row.partner_a,
                "receptor_partner": row.partner_b,
                "ligand_is_complex": ligand_complex,
                "receptor_is_complex": receptor_complex,
                "classification": row.classification,
                "directionality": row.directionality,
                "annotation_strategy": row.annotation_strategy,
                "evidence": row.source,
            })

pd.DataFrame.from_records(records).to_parquet(
    stage / "lr_pairs.parquet", index=False, engine="pyarrow", compression="zstd"
)
PY

  local package=$output_root/lrdb.cellphonedb.v5-v5.0.0
  "${catalog[@]}" build "$stage" "$package" --force \
    --id lrdb.cellphonedb.v5 --version v5.0.0 --kind lr_interaction_library \
    --metadata source=cellphonedb-data-v5.0.0 \
    --metadata license=MIT \
    --metadata license_provenance=OmniPath_resource_info \
    --metadata interactions=2911 \
    --metadata description="CellPhoneDB v5 interactions, proteins, genes, complexes, TF links, and normalized LR pairs." \
    --payload '{"raw_interactions":"interaction_input.parquet","lr_table":"lr_pairs.parquet","compiled_database":"source/cellphonedb.zip"}'
  "${catalog[@]}" validate "$package"
}

build_ramilowski() {
  local stage=$download_root/ramilowski2015-omnipath
  rm -rf "$stage"
  mkdir -p "$stage"
  fetch \
    'https://omnipathdb.org/interactions/?datasets=ligrecextra&resources=Ramilowski2015&fields=sources,databases,entity_type&genesymbols=1' \
    "$stage/interactions.tsv"
  write_ramilowski_readme "$stage/README.md"

  python - "$stage" <<'PY'
from pathlib import Path
import sys

import pandas as pd

stage = Path(sys.argv[1])
raw = pd.read_csv(stage / "interactions.tsv", sep="\t", dtype=str, keep_default_na=False)
raw.to_parquet(
    stage / "interactions.parquet", index=False, engine="pyarrow", compression="zstd"
)
lr = pd.DataFrame({
    "ligand_uniprot": raw["source"],
    "receptor_uniprot": raw["target"],
    "ligand": raw["source_genesymbol"],
    "receptor": raw["target_genesymbol"],
    "is_directed": raw["is_directed"],
    "is_stimulation": raw["is_stimulation"],
    "is_inhibition": raw["is_inhibition"],
    "evidence_sources": raw["sources"],
})
lr.to_parquet(
    stage / "lr_pairs.parquet", index=False, engine="pyarrow", compression="zstd"
)
PY

  local package=$output_root/lrdb.ramilowski2015-v2015-omnipath-2026.09.17
  "${catalog[@]}" build "$stage" "$package" --force \
    --id lrdb.ramilowski2015 --version v2015-omnipath-2026.09.17 --kind lr_interaction_library \
    --metadata source=Ramilowski_2015_via_OmniPath_ligrecextra \
    --metadata retrieved_at=2026-09-17 \
    --metadata license=CC-BY-4.0 \
    --metadata license_provenance=OmniPath_resource_info \
    --metadata citation='Ramilowski et al., Nature Communications 6:7866 (2015)' \
    --metadata description="Ramilowski 2015 ligand-receptor pairs obtained from OmniPath ligrecextra." \
    --payload '{"raw_interactions":"interactions.parquet","lr_table":"lr_pairs.parquet"}'
  "${catalog[@]}" validate "$package"
}

build_hgnc() {
  local stage=$download_root/hgnc-symbols
  rm -rf "$stage"
  mkdir -p "$stage"
  fetch \
    https://storage.googleapis.com/public-download-files/hgnc/tsv/tsv/hgnc_complete_set.txt \
    "$stage/hgnc_complete_set.tsv"
  write_hgnc_readme "$stage/README.md"

  python - "$stage" <<'PY'
from pathlib import Path
import sys

import pandas as pd

stage = Path(sys.argv[1])
raw = pd.read_csv(
    stage / "hgnc_complete_set.tsv", sep="\t", dtype=str, keep_default_na=False
)
raw.to_parquet(
    stage / "hgnc_complete_set.parquet",
    index=False,
    engine="pyarrow",
    compression="zstd",
)

records = []
for row in raw.itertuples(index=False):
    symbol = getattr(row, "symbol", "")
    hgnc_id = getattr(row, "hgnc_id", "")
    for field, alias_type in (("alias_symbol", "alias"), ("prev_symbol", "previous")):
        for alias in filter(None, map(str.strip, getattr(row, field, "").split(","))):
            records.append({
                "hgnc_id": hgnc_id,
                "symbol": symbol,
                "alias_symbol": alias,
                "alias_type": alias_type,
            })
pd.DataFrame.from_records(records).to_parquet(
    stage / "alias_dictionary.parquet",
    index=False,
    engine="pyarrow",
    compression="zstd",
)
PY

  local package=$output_root/genecards.hgnc_symbols-v2026.09.15
  "${catalog[@]}" build "$stage" "$package" --force \
    --id genecards.hgnc_symbols --version v2026.09.15 --kind gene_alias_dictionary \
    --metadata source=HGNC_complete_set \
    --metadata access_use_policy=no_restrictions_attribution_requested \
    --metadata source_last_modified=2026-09-15 \
    --metadata description="HGNC approved symbols plus current and previous alias dictionary in Parquet." \
    --payload '{"complete_set":"hgnc_complete_set.parquet","alias_dictionary":"alias_dictionary.parquet"}'
  "${catalog[@]}" validate "$package"
}

build_celltypist() {
  local stage=$download_root/celltypist-pan-immune-v2
  rm -rf "$stage"
  mkdir -p "$stage"
  fetch https://celltypist.cog.sanger.ac.uk/models/models.json "$stage/models.json"
  for model in Immune_All_Low.pkl Immune_All_High.pkl; do
    fetch "https://celltypist.cog.sanger.ac.uk/models/Pan_Immune_CellTypist/v2/$model" \
      "$stage/$model"
  done
  python - "$stage" <<'PY'
from pathlib import Path
import json
import sys

stage = Path(sys.argv[1])
source = json.loads((stage / "models.json").read_text(encoding="utf-8"))
selected = {
    "description": "Selected CellTypist Pan-immune v2 models",
    "models": [
        model for model in source["models"]
        if model["filename"] in {"Immune_All_Low.pkl", "Immune_All_High.pkl"}
    ],
}
(stage / "selected_models.json").write_text(
    json.dumps(selected, indent=2, sort_keys=True) + "\n", encoding="utf-8"
)
(stage / "models.json").unlink()
PY
  write_celltypist_readme "$stage/README.md"

  local package=$output_root/celltypist.models.pan_immune-v2
  "${catalog[@]}" build "$stage" "$package" --force \
    --id celltypist.models.pan_immune --version v2 --kind celltypist_model \
    --metadata source=CellTypist_official_model_service \
    --metadata license=unspecified_official_model_service \
    --metadata model_collection=Pan_Immune_CellTypist \
    --metadata model_list_date=2026-03-16 \
    --metadata citation='Dominguez Conde et al., Science 376:eabl5197 (2022)' \
    --metadata description="Official CellTypist Pan-immune v2 low- and high-resolution models." \
    --payload '{"default_model":"Immune_All_Low.pkl","models":"selected_models.json"}'
  "${catalog[@]}" validate "$package"
}

build_cellphonedb
build_ramilowski
build_hgnc
build_celltypist

if [[ "$publish" == 1 ]]; then
  for package in \
    "$output_root/lrdb.cellphonedb.v5-v5.0.0" \
    "$output_root/lrdb.ramilowski2015-v2015-omnipath-2026.09.17" \
    "$output_root/genecards.hgnc_symbols-v2026.09.15" \
    "$output_root/celltypist.models.pan_immune-v2"
  do
    "${catalog[@]}" publish "$package" --config "$config"
  done
fi

echo "Single-cell reference packages are ready under $output_root"
