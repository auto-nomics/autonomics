#!/usr/bin/env Rscript
# Generate golden reference outputs from R rdrobust + rdpower.
# Outputs key-value pairs for cross-validation.

library(rdrobust)
library(rdpower)

# Load Senate data
data_senate <- read.csv("reference/rdpower/R/rdpower_senate.csv")
ok <- complete.cases(data_senate$demvoteshfor2, data_senate$demmv)
Y <- data_senate$demvoteshfor2[ok]
R <- data_senate$demmv[ok]

cat("n_obs:", length(Y), "\n")

# --- rdrobust: default settings
rd <- rdrobust(Y, R, c=0, p=1, deriv=0, kernel="triangular",
               bwselect="mserd", vce="nn", level=95, stdvars=FALSE)

cat("tau_cl:",   sprintf("%.15e", rd$Estimate[1, "tau.us"]), "\n")
cat("tau_bc:",   sprintf("%.15e", rd$Estimate[1, "tau.bc"]), "\n")
cat("se_cl:",    sprintf("%.15e", rd$Estimate[1, "se.us"]), "\n")
cat("se_rb:",    sprintf("%.15e", rd$Estimate[1, "se.rb"]), "\n")
cat("h_l:",      sprintf("%.15e", rd$bws[1, 1]), "\n")
cat("h_r:",      sprintf("%.15e", rd$bws[1, 2]), "\n")
cat("b_l:",      sprintf("%.15e", rd$bws[2, 1]), "\n")
cat("b_r:",      sprintf("%.15e", rd$bws[2, 2]), "\n")
cat("n_l:",      rd$N[1], "\n")
cat("n_r:",      rd$N[2], "\n")
cat("n_h_l:",    rd$N_h[1], "\n")
cat("n_h_r:",    rd$N_h[2], "\n")
cat("bias_l:",   sprintf("%.15e", rd$bias[1]), "\n")
cat("bias_r:",   sprintf("%.15e", rd$bias[2]), "\n")
cat("V_cl_l_11:", sprintf("%.15e", rd$V_cl_l[1,1]), "\n")
cat("V_cl_r_11:", sprintf("%.15e", rd$V_cl_r[1,1]), "\n")
cat("V_rb_l_11:", sprintf("%.15e", rd$V_rb_l[1,1]), "\n")
cat("V_rb_r_11:", sprintf("%.15e", rd$V_rb_r[1,1]), "\n")
cat("bwselect:",  rd$bwselect, "\n")
cat("kernel:",    rd$kernel, "\n")
cat("vce:",       rd$vce, "\n")

# --- rdpower with tau=5
pd <- rdpower(data=cbind(Y,R), tau=5, cutoff=0, p=1, kernel="triangular",
              bwselect="mserd", vce="nn", level=95)
cat("\n--- rdpower tau=5 ---\n")
cat("power_rbc:",  sprintf("%.15e", pd$power.rbc), "\n")
cat("power_conv:", sprintf("%.15e", pd$power.conv), "\n")
cat("se_rbc:",     sprintf("%.15e", pd$se.rbc), "\n")
cat("se_conv:",    sprintf("%.15e", pd$se.conv), "\n")
cat("samph_l:",    sprintf("%.15e", pd$samph.l), "\n")
cat("samph_r:",    sprintf("%.15e", pd$samph.r), "\n")
cat("sampsi_l:",   pd$sampsi.l, "\n")
cat("sampsi_r:",   pd$sampsi.r, "\n")

# --- rdsampsi with tau=5, beta=0.8
ss <- rdsampsi(data=cbind(Y,R), tau=5, cutoff=0, beta=0.8, p=1,
               kernel="triangular", bwselect="mserd", vce="nn", level=95)
cat("\n--- rdsampsi tau=5 beta=0.8 ---\n")
cat("sampsi_h_tot:",  ss$sampsi.h.tot, "\n")
cat("sampsi_h_l:",    ss$sampsi.h.l, "\n")
cat("sampsi_h_r:",    ss$sampsi.h.r, "\n")
cat("nratio:",        sprintf("%.15e", ss$sampsi.h.tot), "\n")
