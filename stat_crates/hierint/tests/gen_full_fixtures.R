#!/usr/bin/env Rscript
# Comprehensive golden fixtures for full end-to-end cross-validation.
# Includes coefficients, predictions, and CV results for exact comparison.
library(jsonlite)
library(glinternet)
library(hierNet)
outdir <- "."

flatten <- function(m) as.numeric(m)

# ═══════════════════════════════════════════════════════════════════════
# Helper: extract glinternet coefficients at each lambda
# ═══════════════════════════════════════════════════════════════════════
extract_glinternet_coefs <- function(fit, numLevels) {
  coefs <- coef(fit)
  result <- list()
  for (i in seq_along(coefs)) {
    c <- coefs[[i]]
    result[[i]] <- list(
      mainEffectsCat = if(is.null(c$mainEffects$cat)) list() else c$mainEffects$cat,
      mainEffectsCont = if(is.null(c$mainEffects$cont)) list() else c$mainEffects$cont,
      interactionsCatCat = if(is.null(c$interactions$catcat)) list() else c$interactions$catcat,
      interactionsContCont = if(is.null(c$interactions$contcont)) list() else c$interactions$contcont,
      interactionsCatCont = if(is.null(c$interactions$catcont)) list() else c$interactions$catcont,
      mainEffectsCoefCat = if(is.null(c$mainEffectsCoef$cat)) list() else lapply(c$mainEffectsCoef$cat, function(x) as.numeric(x)),
      mainEffectsCoefCont = if(is.null(c$mainEffectsCoef$cont)) list() else as.numeric(c$mainEffectsCoef$cont),
      interactionsCoefCatCat = if(is.null(c$interactionsCoef$catcat)) list() else lapply(c$interactionsCoef$catcat, function(x) as.numeric(x)),
      interactionsCoefContCont = if(is.null(c$interactionsCoef$contcont)) list() else lapply(c$interactionsCoef$contcont, function(x) as.numeric(x)),
      interactionsCoefCatCont = if(is.null(c$interactionsCoef$catcont)) list() else lapply(c$interactionsCoef$catcont, function(x) as.numeric(x))
    )
  }
  result
}

# ═══════════════════════════════════════════════════════════════════════
# Fixture A: glinternet Gaussian continuous — coefficients + predictions
# ═══════════════════════════════════════════════════════════════════════
gen_glinternet_gauss_full <- function() {
  set.seed(42)
  n <- 200; p <- 10
  X <- matrix(rnorm(n * p), n, p)
  # Known structure: main effects x1, x2; interaction x3*x4
  y <- 2*X[,1] - 1.5*X[,2] + 1.0*X[,3]*X[,4] + rnorm(n, sd=0.3)
  numLevels <- rep(1, p)

  nLambda <- 25
  fit <- glinternet(X, y, numLevels, nLambda=nLambda)
  preds <- predict(fit, X)

  # For each lambda, extract active set details
  activeDetails <- lapply(fit$activeSet, function(as) {
    list(
      cat = if(is.null(as$cat)) NULL else as.vector(as$cat),
      cont = if(is.null(as$cont)) NULL else as.vector(as$cont),
      catcat = if(is.null(as$catcat)) NULL else as.vector(t(as$catcat)),
      contcont = if(is.null(as$contcont)) NULL else as.vector(t(as$contcont)),
      catcont = if(is.null(as$catcont)) NULL else as.vector(t(as$catcont))
    )
  })

  list(
    name = "glinternet_gauss_full",
    X = flatten(X), y = y, numLevels = numLevels, n = n, p = p,
    lambda = fit$lambda,
    objValue = fit$objValue,
    fitted = flatten(fit$fitted),
    predictions = flatten(preds),
    activeDetails = activeDetails
  )
}

# ═══════════════════════════════════════════════════════════════════════
# Fixture B: glinternet logistic categorical — coefficients + predictions
# ═══════════════════════════════════════════════════════════════════════
gen_glinternet_logit_full <- function() {
  set.seed(123)
  n <- 300; p <- 6
  X <- matrix(sample(0:2, n * p, replace=TRUE), n, p)
  numLevels <- rep(3, p)
  # Create response with known structure
  eta <- 0.5 * (X[,1]==1) - 0.5 * (X[,1]==2) + 0.8 * (X[,2]==0) * (X[,3]==1)
  y <- rbinom(n, 1, plogis(eta))

  nLambda <- 20
  fit <- glinternet(X, y, numLevels, family="binomial", nLambda=nLambda)
  preds <- predict(fit, X)

  activeDetails <- lapply(fit$activeSet, function(as) {
    list(
      cat = if(is.null(as$cat)) NULL else as.vector(as$cat),
      cont = if(is.null(as$cont)) NULL else as.vector(as$cont),
      catcat = if(is.null(as$catcat)) NULL else as.vector(t(as$catcat)),
      contcont = if(is.null(as$contcont)) NULL else as.vector(t(as$contcont)),
      catcont = if(is.null(as$catcont)) NULL else as.vector(t(as$catcont))
    )
  })

  list(
    name = "glinternet_logit_full",
    X = flatten(X), y = as.numeric(y), numLevels = numLevels, n = n, p = p,
    lambda = fit$lambda,
    objValue = fit$objValue,
    fitted = flatten(fit$fitted),
    predictions = flatten(preds),
    activeDetails = activeDetails
  )
}

# ═══════════════════════════════════════════════════════════════════════
# Fixture C: glinternet mixed (2 cat + 3 cont) — coefficients + predictions
# ═══════════════════════════════════════════════════════════════════════
gen_glinternet_mixed_full <- function() {
  set.seed(77)
  n <- 250
  numLevels <- c(3, 2, 1, 1, 1)
  p <- length(numLevels)
  X <- sapply(1:p, function(j) {
    if (numLevels[j] > 1) sample(0:(numLevels[j]-1), n, replace=TRUE)
    else rnorm(n)
  })

  eta <- 0.8*X[,3] + 0.5*X[,4] + 0.6*(X[,1]==1)*X[,3]
  y <- eta + rnorm(n, sd=0.4)

  nLambda <- 20
  fit <- glinternet(X, y, numLevels, nLambda=nLambda)
  preds <- predict(fit, X)

  activeDetails <- lapply(fit$activeSet, function(as) {
    list(
      cat = if(is.null(as$cat)) NULL else as.vector(as$cat),
      cont = if(is.null(as$cont)) NULL else as.vector(as$cont),
      catcat = if(is.null(as$catcat)) NULL else as.vector(t(as$catcat)),
      contcont = if(is.null(as$contcont)) NULL else as.vector(t(as$contcont)),
      catcont = if(is.null(as$catcont)) NULL else as.vector(t(as$catcont))
    )
  })

  list(
    name = "glinternet_mixed_full",
    X = flatten(X), y = y, numLevels = numLevels, n = n, p = p,
    lambda = fit$lambda,
    objValue = fit$objValue,
    fitted = flatten(fit$fitted),
    predictions = flatten(preds),
    activeDetails = activeDetails
  )
}

# ═══════════════════════════════════════════════════════════════════════
# Fixture D: hierNet Gaussian weak — full coefficients at each lambda
# ═══════════════════════════════════════════════════════════════════════
gen_hiernet_weak_full <- function() {
  set.seed(42)
  n <- 100; p <- 8
  x <- matrix(rnorm(n * p), n, p)
  y <- x[,1] + 0.5*x[,2] + 0.8*x[,1]*x[,3] + rnorm(n, sd=0.3)

  fit <- hierNet.path(x, y, strong=FALSE, diagonal=FALSE, nlam=10, maxiter=500)
  preds <- predict(fit, x)

  list(
    name = "hiernet_weak_full",
    x = flatten(x), y = y, n = n, p = p,
    strong = FALSE, diagonal = FALSE,
    lamlist = fit$lamlist,
    bp = flatten(fit$bp),   # p × nlam column-major
    bn = flatten(fit$bn),
    th = flatten(fit$th),   # p × p × nlam
    obj = fit$obj,
    predictions = flatten(preds)  # n × nlam
  )
}

# ═══════════════════════════════════════════════════════════════════════
# Fixture E: hierNet Gaussian strong — full coefficients
# ═══════════════════════════════════════════════════════════════════════
gen_hiernet_strong_full <- function() {
  set.seed(42)
  n <- 100; p <- 6
  x <- matrix(rnorm(n * p), n, p)
  y <- x[,1] + x[,2] + x[,1]*x[,2] + rnorm(n, sd=0.3)

  fit <- hierNet.path(x, y, strong=TRUE, diagonal=FALSE, nlam=8, niter=50, maxiter=500)
  preds <- predict(fit, x)

  list(
    name = "hiernet_strong_full",
    x = flatten(x), y = y, n = n, p = p,
    strong = TRUE, diagonal = FALSE,
    lamlist = fit$lamlist,
    bp = flatten(fit$bp),
    bn = flatten(fit$bn),
    th = flatten(fit$th),
    obj = fit$obj,
    predictions = flatten(preds)
  )
}

# ═══════════════════════════════════════════════════════════════════════
# Fixture F: glinternet CV — full CV path
# ═══════════════════════════════════════════════════════════════════════
gen_glinternet_cv <- function() {
  set.seed(42)
  n <- 150; p <- 8
  X <- matrix(rnorm(n * p), n, p)
  y <- X[,1] + X[,2]*X[,3] + rnorm(n, sd=0.5)
  numLevels <- rep(1, p)

  cv <- glinternet.cv(X, y, numLevels, nFolds=5, nLambda=15)

  list(
    name = "glinternet_cv",
    X = flatten(X), y = y, numLevels = numLevels, n = n, p = p,
    lambda = cv$lambda,
    lambdaHat = cv$lambdaHat,
    lambdaHat1Std = cv$lambdaHat1Std,
    cvErr = cv$cvErr,
    cvErrStd = cv$cvErrStd
  )
}

# ═══════════════════════════════════════════════════════════════════════
# Fixture G: Edge case — single continuous variable
# ═══════════════════════════════════════════════════════════════════════
gen_edge_single_var <- function() {
  set.seed(42)
  n <- 50; p <- 1
  X <- matrix(rnorm(n), n, p)
  y <- 2*X[,1] + rnorm(n, sd=0.3)
  numLevels <- rep(1, p)

  fit <- glinternet(X, y, numLevels, nLambda=10)
  preds <- predict(fit, X)

  list(
    name = "edge_single_var",
    X = flatten(X), y = y, numLevels = numLevels, n = n, p = p,
    lambda = fit$lambda,
    objValue = fit$objValue,
    fitted = flatten(fit$fitted),
    predictions = flatten(preds)
  )
}

# ═══════════════════════════════════════════════════════════════════════
# Fixture H: Edge case — all categorical binary
# ═══════════════════════════════════════════════════════════════════════
gen_edge_binary_cat <- function() {
  set.seed(42)
  n <- 100; p <- 5
  X <- matrix(sample(0:1, n * p, replace=TRUE), n, p)
  numLevels <- rep(2, p)
  y <- rnorm(n)

  fit <- glinternet(X, y, numLevels, nLambda=10)
  preds <- predict(fit, X)

  list(
    name = "edge_binary_cat",
    X = flatten(X), y = y, numLevels = numLevels, n = n, p = p,
    lambda = fit$lambda,
    objValue = fit$objValue,
    fitted = flatten(fit$fitted),
    predictions = flatten(preds)
  )
}

# ═══════════════════════════════════════════════════════════════════════
# Generate all
# ═══════════════════════════════════════════════════════════════════════
fixtures <- list(
  A = gen_glinternet_gauss_full(),
  B = gen_glinternet_logit_full(),
  C = gen_glinternet_mixed_full(),
  D = gen_hiernet_weak_full(),
  E = gen_hiernet_strong_full(),
  F = gen_glinternet_cv(),
  G = gen_edge_single_var(),
  H = gen_edge_binary_cat()
)

for (name in names(fixtures)) {
  fname <- paste0(fixtures[[name]]$name, ".json")
  cat("Writing", fname, "\n")
  write_json(fixtures[[name]], file.path(outdir, fname), digits=14, auto_unbox=TRUE)
}

cat("Done.\n")
