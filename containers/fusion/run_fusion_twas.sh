#!/bin/sh
set -eu
cd /opt/fusion

weights_root=/panels/fusion_ref/weights
ldref_root=/panels/fusion_ref/LDREF
archive_name=${FUSION_TISSUE_ARCHIVE:?FUSION_TISSUE_ARCHIVE is required}
weights_archive="${weights_root}/${archive_name}"
pos_suffix=$([ "${FUSION_USE_NOFILTER_WEIGHTS:-0}" = "1" ] && printf nofilter.pos || printf pos)

[ -f "${AUTONOMICS_INPUT0:?AUTONOMICS_INPUT0 is required}" ]
[ -f "${weights_archive}" ] || {
    echo "FUSION GTEx weight archive is missing: ${archive_name}" >&2
    exit 1
}
[ -f "${ldref_root}/1000G.EUR.${FUSION_CHR}.bim" ] || {
    echo "FUSION LDREF chromosome ${FUSION_CHR} is missing" >&2
    exit 1
}

mkdir -p /work/weights
tar --no-same-owner -xzf "${weights_archive}" -C /work/weights
tissue_dir=$(tar -tzf "${weights_archive}" | sed -n '/\/$/s/\/$//p' | head -1)
weights_pos="/work/weights/${tissue_dir}.${pos_suffix}"
[ -f "${weights_pos}" ] || {
    echo "FUSION weight manifest is missing: ${tissue_dir}.${pos_suffix}" >&2
    exit 1
}

model_args=
if [ -n "${FUSION_FORCE_MODEL:-}" ]; then
    model_args="--force_model ${FUSION_FORCE_MODEL}"
fi
perm_args=
if [ "${FUSION_PERM:-0}" -gt 0 ]; then
    perm_args="--perm ${FUSION_PERM} --perm_minp ${FUSION_PERM_MINP}"
fi

out_prefix=/work/twas_fusion
: > "${AUTONOMICS_OUTPUT2}"
Rscript /opt/fusion/FUSION.assoc_test.R \
    --sumstats "${AUTONOMICS_INPUT0}" \
    --weights "${weights_pos}" \
    --weights_dir /work/weights \
    --ref_ld_chr "${ldref_root}/1000G.EUR." \
    --chr "${FUSION_CHR}" \
    --max_impute "${FUSION_MAX_IMPUTE}" \
    --min_r2pred "${FUSION_MIN_R2PRED}" \
    ${model_args} \
    ${perm_args} \
    --out "${out_prefix}" \
    > "${AUTONOMICS_OUTPUT1}" 2>&1

cp "${out_prefix}" "${AUTONOMICS_OUTPUT0}"
if [ -f "${out_prefix}.MHC" ]; then
    cp "${out_prefix}.MHC" "${AUTONOMICS_OUTPUT2}"
fi

{
    echo "FUSION TWAS completed."
    echo "tissue_archive: ${archive_name}"
    echo "weight_manifest: ${tissue_dir}.${pos_suffix}"
    echo "chromosome: ${FUSION_CHR}"
} >> "${AUTONOMICS_OUTPUT1}"
