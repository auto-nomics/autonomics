## Golden generator for tests/rotation_r_reference.rs — R stats::varimax /
## stats::promax parity for the ml::rotation ports.
##
## Run locally (R ≥ 4.x with jsonlite):
##   Rscript stat_crates/ml/tests/golden/gen_rotation_reference.R
suppressPackageStartupMessages(library(jsonlite))

out_dir <- "stat_crates/ml/tests/golden"

## 15×3 loading matrix: two clean factors + a diffuse third.
set.seed(20260926)
lam <- cbind(
  c(0.85, 0.82, 0.78, 0.74, 0.70, 0.12, 0.15, 0.10, 0.18, 0.08, 0.55, 0.48, 0.44, 0.10, 0.62),
  c(0.10, 0.14, 0.18, 0.09, 0.12, 0.80, 0.76, 0.72, 0.68, 0.64, 0.42, 0.10, 0.50, 0.66, 0.20),
  c(0.12, 0.30, 0.25, 0.35, 0.22, 0.15, 0.30, 0.20, 0.10, 0.05, 0.28, 0.33, 0.15, 0.40, 0.30)
)
lam <- lam + matrix(rnorm(15 * 3, 0, 0.03), 15, 3)

## 6×2 small matrix (the unit-test fixture) for direct parity too.
small <- rbind(
  c(0.90, 0.90), c(0.83, 0.86), c(0.10, 0.70), c(0.15, 0.65),
  c(0.85, 0.20), c(0.72, 0.64)
)

flat <- function(m) as.numeric(t(m))  # jsonlite flattens column-major

emit <- function(x, key) {
  vm <- varimax(x, normalize = TRUE, eps = 1e-5)
  vm0 <- varimax(x, normalize = FALSE, eps = 1e-5)
  pm <- promax(x, m = 4)
  list(
    loadings = flat(x),
    varimax = list(loadings = flat(vm$loadings), rotmat = flat(vm$rotmat)),
    varimax_unnorm = list(loadings = flat(vm0$loadings), rotmat = flat(vm0$rotmat)),
    promax = list(loadings = flat(pm$loadings), rotmat = flat(pm$rotmat))
  )
}

ref <- list(
  rows = list(main = nrow(lam), small = nrow(small)),
  cols = list(main = ncol(lam), small = ncol(small)),
  main = emit(lam, "main"),
  small = emit(small, "small")
)
write.csv(lam, file.path(out_dir, "rotation_loadings.csv"), row.names = FALSE)
write_json(ref, file.path(out_dir, "rotation_reference.json"), auto_unbox = TRUE, digits = 16)
cat("regenerated", out_dir, "goldens\n")
