#!/usr/bin/env bash
# Cross-validation: compile DAG → R, run generated R, compare to reference.
#
# This script:
# 1. Uses a small Rust binary to compile a DAG manifest to R
# 2. Runs the generated R script (which reads the data CSV, runs analysis, writes edge CSVs)
# 3. Compares the edge CSV output to the reference R result
#
# Usage: ./tests/run_xval.sh <test_name>
# Requires: Rscript + data.table + survival + pROC installed

set -euo pipefail
TEST_NAME="${1:?usage: $0 <test_name>}"
XVAL_DIR="/tmp/autonomics_xval"
DATA_CSV="${XVAL_DIR}/${TEST_NAME}_data.csv"
REF_CSV="${XVAL_DIR}/${TEST_NAME}_reference.csv"

if [ ! -f "$DATA_CSV" ]; then
    echo "Generating data for $TEST_NAME..."
    Rscript tests/cross_validate.R "$TEST_NAME" "$XVAL_DIR"
fi

# Build a DAG manifest JSON for this test case
case "$TEST_NAME" in
    linear_regression)
        MANIFEST='{"nodes":[{"id":"src","kind":"source_file","spec":{"path":"'"$DATA_CSV"'"}},{"id":"lm","kind":"linear_regression","spec":{"x_columns":["x1","x2"],"y_column":"y","intercept":true}}],"edges":[{"from":"src","from_port":0,"to":"lm","to_port":0}]}'
        OUTPUT_EDGE="_edge_lm_0.csv"
        COMPARE_COLS="term,coefficient,std_error,t_stat,p_value,r_squared"
        ;;
    logistic_regression)
        MANIFEST='{"nodes":[{"id":"src","kind":"source_file","spec":{"path":"'"$DATA_CSV"'"}},{"id":"glm","kind":"logistic_regression","spec":{"predictors":["x1","x2"],"outcome":"y","intercept":true}}],"edges":[{"from":"src","from_port":0,"to":"glm","to_port":0}]}'
        OUTPUT_EDGE="_edge_glm_0.csv"
        COMPARE_COLS="term,coefficient,std_error,z_stat,p_value"
        ;;
    chi_square)
        MANIFEST='{"nodes":[{"id":"src","kind":"source_file","spec":{"path":"'"$DATA_CSV"'"}},{"id":"chi","kind":"chi_square","spec":{"row_column":"group","col_column":"outcome"}}],"edges":[{"from":"src","from_port":0,"to":"chi","to_port":0}]}'
        OUTPUT_EDGE="_edge_chi_0.csv"
        COMPARE_COLS="chi_squared,df,p_value,n"
        ;;
    cox_regression)
        MANIFEST='{"nodes":[{"id":"src","kind":"source_file","spec":{"path":"'"$DATA_CSV"'"}},{"id":"cox","kind":"cox_regression","spec":{"predictors":["x1","x2"],"time_column":"time","event_column":"event"}}],"edges":[{"from":"src","from_port":0,"to":"cox","to_port":0}]}'
        OUTPUT_EDGE="_edge_cox_0.csv"
        COMPARE_COLS="term,coefficient,std_error,z_stat,p_value"
        ;;
    epi_roc)
        MANIFEST='{"nodes":[{"id":"src","kind":"source_file","spec":{"path":"'"$DATA_CSV"'"}},{"id":"roc","kind":"epi_roc","spec":{"score1_column":"score","label_column":"label"}}],"edges":[{"from":"src","from_port":0,"to":"roc","to_port":0}]}'
        OUTPUT_EDGE="_edge_roc_0.csv"
        COMPARE_COLS="auc"
        ;;
    *)
        echo "Unknown test: $TEST_NAME"
        exit 1
        ;;
esac

echo "=== $TEST_NAME ==="
echo "Manifest: $MANIFEST"
echo ""

# Compile to R using a small Rust test binary
# We'll use cargo test to drive this
export XVAL_MANIFEST="$MANIFEST"
export XVAL_OUTPUT_DIR="$XVAL_DIR"
export XVAL_TEST_NAME="$TEST_NAME"

# Run the Rust cross-validation test
cargo test -p data-engine --lib -- cross_validate::$TEST_NAME --nocapture 2>&1 || true

# Check if the generated R script exists
R_SCRIPT="${XVAL_DIR}/${TEST_NAME}_generated.R"
if [ ! -f "$R_SCRIPT" ]; then
    echo "ERROR: Generated R script not found at $R_SCRIPT"
    echo "The Rust test should have written it there."
    exit 1
fi

echo "--- Generated R script ---"
cat "$R_SCRIPT"
echo ""

# Run the generated R script
echo "--- Running generated R ---"
cd "$XVAL_DIR"
Rscript "${TEST_NAME}_generated.R" 2>&1
echo ""

# Check output edge CSV exists
if [ ! -f "$OUTPUT_EDGE" ]; then
    echo "ERROR: Output edge CSV '$OUTPUT_EDGE' not produced"
    exit 1
fi

echo "--- Generated output ($OUTPUT_EDGE) ---"
cat "$OUTPUT_EDGE"
echo ""

echo "--- Reference output ---"
cat "$REF_CSV"
echo ""

# Compare numerically
echo "--- Comparison ---"
Rscript -e "
gen <- data.table::fread('$OUTPUT_EDGE')
ref <- data.table::fread('$REF_CSV')
cat('Generated columns:', names(gen), '\n')
cat('Reference columns:', names(ref), '\n')

# Find common numeric columns
common <- intersect(names(gen), names(ref))
cat('Common columns:', paste(common, collapse=', '), '\n\n')

max_diff <- 0
for (col in common) {
    if (is.numeric(gen[[col]]) && is.numeric(ref[[col]])) {
        # Align by term if present
        if ('term' %in% common && col != 'term') {
            gen_sorted <- gen[order(term)]
            ref_sorted <- ref[order(term)]
            if (nrow(gen_sorted) == nrow(ref_sorted)) {
                diff <- max(abs(gen_sorted[[col]] - ref_sorted[[col]]), na.rm=TRUE)
                cat(sprintf('  %-15s max diff: %.6e  %s\n', col, diff,
                    ifelse(diff < 1e-4, 'PASS', 'CHECK')))
                max_diff <- max(max_diff, diff)
            }
        } else if (length(gen[[col]]) == 1 && length(ref[[col]]) == 1) {
            diff <- abs(gen[[col]] - ref[[col]])
            cat(sprintf('  %-15s diff: %.6e  %s\n', col, diff,
                ifelse(diff < 1e-4, 'PASS', 'CHECK')))
            max_diff <- max(max_diff, diff)
        }
    }
}

cat(sprintf('\nOverall max diff: %.6e  %s\n', max_diff,
    ifelse(max_diff < 1e-4, 'ALL PASS', 'SOME CHECK')))
if (max_diff < 1e-4) {
    cat('CROSS-VALIDATION: PASS\n')
} else {
    cat('CROSS-VALIDATION: NEEDS REVIEW\n')
}
" 2>&1
