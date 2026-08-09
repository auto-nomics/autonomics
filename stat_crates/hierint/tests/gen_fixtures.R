#!/usr/bin/env Rscript
# Generate golden test fixtures for hierint crate cross-validation.
# Run: Rscript gen_fixtures.R
library(jsonlite)
library(glinternet)
library(hierNet)

set.seed(42)
outdir <- "."

# ═══════════════════════════════════════════════════════════════════════
# Helper: serialize matrix as column-major flat array
# ═══════════════════════════════════════════════════════════════════════
flatten <- function(m) as.numeric(m)  # R matrices are already column-major

# ═══════════════════════════════════════════════════════════════════════
# Fixture 1: glinternet Gaussian, continuous only
# ═══════════════════════════════════════════════════════════════════════
gen_glinternet_gaussian <- function() {
  set.seed(42)
  n <- 200; p <- 10
  X <- matrix(rnorm(n * p), n, p)
  # True model: y = x1 + x2 + x3*x4 + noise
  y <- X[,1] + X[,2] + X[,3]*X[,4] + rnorm(n, sd=0.5)
  numLevels <- rep(1, p)

  fit <- glinternet(X, y, numLevels, nLambda=20)

  list(
    name = "glinternet_gaussian",
    X = flatten(X),
    y = y,
    numLevels = numLevels,
    n = n,
    p = p,
    lambda = fit$lambda,
    objValue = fit$objValue,
    activeSet = lapply(fit$activeSet, function(as) {
      list(
        cat = if(is.null(as$cat)) NULL else as$cat,
        cont = if(is.null(as$cont)) NULL else as$cont,
        catcat = if(is.null(as$catcat)) NULL else as$catcat,
        contcont = if(is.null(as$contcont)) NULL else as$contcont,
        catcont = if(is.null(as$catcont)) NULL else as$catcont
      )
    }),
    betahat = fit$betahat,
    fitted = flatten(fit$fitted)
  )
}

# ═══════════════════════════════════════════════════════════════════════
# Fixture 2: glinternet logistic, categorical
# ═══════════════════════════════════════════════════════════════════════
gen_glinternet_logistic <- function() {
  set.seed(42)
  n <- 300; p <- 8
  X <- matrix(sample(0:2, n * p, replace=TRUE), n, p)
  numLevels <- rep(3, p)
  y <- rbinom(n, 1, 0.5)

  fit <- glinternet(X, y, numLevels, family="binomial", nLambda=15)

  list(
    name = "glinternet_logistic",
    X = flatten(X),  # stored as numeric (0,1,2 codes)
    y = as.numeric(y),
    numLevels = numLevels,
    n = n,
    p = p,
    lambda = fit$lambda,
    objValue = fit$objValue,
    activeSet = lapply(fit$activeSet, function(as) {
      list(
        cat = if(is.null(as$cat)) NULL else as$cat,
        cont = if(is.null(as$cont)) NULL else as$cont,
        catcat = if(is.null(as$catcat)) NULL else as$catcat,
        contcont = if(is.null(as$contcont)) NULL else as$contcont,
        catcont = if(is.null(as$catcont)) NULL else as$catcont
      )
    }),
    betahat = fit$betahat,
    fitted = flatten(fit$fitted)
  )
}

# ═══════════════════════════════════════════════════════════════════════
# Fixture 3: glinternet mixed (cat + cont)
# ═══════════════════════════════════════════════════════════════════════
gen_glinternet_mixed <- function() {
  set.seed(42)
  n <- 200
  numLevels <- c(3, 2, 1, 1, 1)  # 2 categorical, 3 continuous
  p <- length(numLevels)
  X <- sapply(1:p, function(j) {
    if (numLevels[j] > 1) sample(0:(numLevels[j]-1), n, replace=TRUE)
    else rnorm(n)
  })
  y <- rnorm(n)

  fit <- glinternet(X, y, numLevels, nLambda=15)

  list(
    name = "glinternet_mixed",
    X = flatten(X),
    y = y,
    numLevels = numLevels,
    n = n,
    p = p,
    lambda = fit$lambda,
    objValue = fit$objValue,
    activeSet = lapply(fit$activeSet, function(as) {
      list(
        cat = if(is.null(as$cat)) NULL else as$cat,
        cont = if(is.null(as$cont)) NULL else as$cont,
        catcat = if(is.null(as$catcat)) NULL else as$catcat,
        contcont = if(is.null(as$contcont)) NULL else as$contcont,
        catcont = if(is.null(as$catcont)) NULL else as$catcont
      )
    }),
    betahat = fit$betahat,
    fitted = flatten(fit$fitted)
  )
}

# ═══════════════════════════════════════════════════════════════════════
# Fixture 4: hierNet Gaussian, weak hierarchy
# ═══════════════════════════════════════════════════════════════════════
gen_hiernet_gaussian <- function() {
  set.seed(42)
  n <- 100; p <- 10
  x <- matrix(rnorm(n * p), n, p)
  y <- x[,1] + x[,2] + x[,1]*x[,3] + rnorm(n, sd=0.5)

  fit <- hierNet.path(x, y, strong=FALSE, diagonal=FALSE, nlam=10, maxiter=500)

  list(
    name = "hiernet_gaussian_weak",
    x = flatten(x),
    y = y,
    n = n,
    p = p,
    strong = FALSE,
    diagonal = FALSE,
    lamlist = fit$lamlist,
    bp = flatten(fit$bp),
    bn = flatten(fit$bn),
    th = flatten(fit$th),
    obj = fit$obj
  )
}

# ═══════════════════════════════════════════════════════════════════════
# Fixture 5: hierNet Gaussian, strong hierarchy
# ═══════════════════════════════════════════════════════════════════════
gen_hiernet_strong <- function() {
  set.seed(42)
  n <- 100; p <- 8
  x <- matrix(rnorm(n * p), n, p)
  y <- x[,1] + x[,2] + x[,1]*x[,2] + rnorm(n, sd=0.5)

  fit <- hierNet.path(x, y, strong=TRUE, diagonal=FALSE, nlam=10, niter=50, maxiter=500)

  list(
    name = "hiernet_gaussian_strong",
    x = flatten(x),
    y = y,
    n = n,
    p = p,
    strong = TRUE,
    diagonal = FALSE,
    lamlist = fit$lamlist,
    bp = flatten(fit$bp),
    bn = flatten(fit$bn),
    th = flatten(fit$th),
    obj = fit$obj
  )
}

# ═══════════════════════════════════════════════════════════════════════
# Generate all fixtures
# ═══════════════════════════════════════════════════════════════════════
cat("Generating glinternet_gaussian...\n")
write_json(gen_glinternet_gaussian(), file.path(outdir, "glinternet_gaussian.json"), digits=12, auto_unbox=TRUE)

cat("Generating glinternet_logistic...\n")
write_json(gen_glinternet_logistic(), file.path(outdir, "glinternet_logistic.json"), digits=12, auto_unbox=TRUE)

cat("Generating glinternet_mixed...\n")
write_json(gen_glinternet_mixed(), file.path(outdir, "glinternet_mixed.json"), digits=12, auto_unbox=TRUE)

cat("Generating hiernet_gaussian_weak...\n")
write_json(gen_hiernet_gaussian(), file.path(outdir, "hiernet_gaussian_weak.json"), digits=12, auto_unbox=TRUE)

cat("Generating hiernet_gaussian_strong...\n")
write_json(gen_hiernet_strong(), file.path(outdir, "hiernet_strong.json"), digits=12, auto_unbox=TRUE)

cat("Done.\n")
