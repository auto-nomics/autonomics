# Golden test data for SuSiE-RSS

## Recovery

The golden JSON files in this directory are archived at:
```
aliyun://autonomics-data/susie/golden/
```

To restore:
```bash
rclone copy aliyun://autonomics-data/susie/golden/ ../../fixtures/golden/
```

## Regeneration

Golden data is generated from susieR 0.16.6 (installed from `reference/susieR/`)
using the exact simulation seeds from susieR's own reference tests.

```bash
# From the workspace root:
Rscript fixtures/gen_golden.R
```

This requires susieR to be installed in the r45 conda env. See
`fixtures/gen_golden.R` for the full scenario matrix (31 scenarios covering
optim/EM/simple methods, z/bhat-shat inputs, with/without n, various L,
prior/residual variance options, purity thresholds, null weights, etc.).
