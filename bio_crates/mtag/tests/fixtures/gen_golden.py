#!/usr/bin/env python3
"""
Generate golden reference outputs for MTAG cross-validation.

This script extracts the CORE mathematical functions from MTAG (gmm_omega,
mtag_analysis, _posDef_adjustment, cov2corr) and runs them on synthetic
scenarios with deterministic inputs. The outputs are saved as JSON for
1:1 comparison with the Rust port.

Run: python3 gen_golden.py
"""

import json
import numpy as np
import numpy.linalg as nla
from scipy.optimize import minimize
from scipy.stats import norm

# ============================================================
# MTAG core math (faithful extraction from mtag.py v1.0.8)
# ============================================================

def gmm_omega(Zs, Ns, sigma_LD):
    """GMM (method of moments) estimator of Omega."""
    N_mats = np.sqrt(np.einsum('mp,mq->mpq', Ns, Ns))
    Z_outer = np.einsum('mp,mq->mpq', Zs, Zs)
    return np.mean((Z_outer - sigma_LD) / N_mats, axis=0)

def _posDef_adjustment(mat, scaling_factor=0.99, max_it=1000):
    """Positive semi-definiteness adjustment."""
    is_pos_semidef = lambda m: np.all(np.linalg.eigvals(m) >= 0)
    if is_pos_semidef(mat):
        return mat
    P = mat.shape[0]
    for i in range(P):
        for j in range(i, P):
            if np.abs(mat[i,j]) > np.sqrt(mat[i,i] * mat[j,j]):
                mat[i,j] = scaling_factor*np.sign(mat[i,j])*np.sqrt(mat[i,i] * mat[j,j])
                mat[j,i] = mat[i,j]
    n = 0
    while not is_pos_semidef(mat) and n < max_it:
        dg = np.diag(mat)
        mat = scaling_factor * mat
        mat[np.diag_indices(P)] = dg
        n += 1
    return mat

def cov2corr(cov):
    """Convert covariance to correlation."""
    std_ = np.sqrt(np.diag(cov))
    corr = cov / np.outer(std_, std_)
    return corr

def mtag_analysis(Zs, Ns, omega_hat, sigma_LD):
    """Core MTAG calculation."""
    M, P = Zs.shape
    W_N = np.einsum('mp,pq->mpq', np.sqrt(Ns), np.eye(P))
    W_N_inv = np.linalg.inv(W_N)
    Sigma_N = np.einsum('mpq,mqr->mpr', np.einsum('mpq,qr->mpr', W_N_inv, sigma_LD), W_N_inv)

    mtag_betas = np.zeros((M, P))
    mtag_se = np.zeros((M, P))
    mtag_factor = np.zeros((M, P))

    for p in range(P):
        gamma_k = omega_hat[:, p]
        tau_k_2 = omega_hat[p, p]
        om_min_gam = omega_hat - np.outer(gamma_k, gamma_k) / tau_k_2

        xx = om_min_gam + Sigma_N
        inv_xx = np.linalg.inv(xx)
        yy = gamma_k / tau_k_2
        W_inv_Z = np.einsum('mqp,mp->mq', W_N_inv, Zs)

        # yy^T · inv_xx for each SNP: out[m,p] = sum_q yy[q] * inv_xx[m,q,p]
        yy_inv_xx = np.tensordot(inv_xx, yy, axes=([1], [0]))  # (M,P)

        # beta_denom = sum_p yy_inv_xx[m,p] * yy[p] → (M,)
        beta_denom = yy_inv_xx @ yy  # (M,)

        mtag_factor[:, p] = yy_inv_xx[:, p] / beta_denom
        mtag_var_p = 1. / beta_denom

        mtag_betas[:, p] = np.sum(yy_inv_xx * W_inv_Z, axis=1) / beta_denom
        mtag_se[:, p] = np.sqrt(mtag_var_p)

    return mtag_betas, mtag_se, mtag_factor

def jointEffect_probability(Z_score, omega_hat, sigma_LD, N_mats):
    """MVN PDF for numerical omega estimation (n_S=1 simplified path)."""
    M, P = Z_score.shape
    cov_s = N_mats * omega_hat[None, :, :] + sigma_LD[None, :, :]  # (M, P, P)
    Ls = np.linalg.cholesky(cov_s)  # (M, P, P)
    # Solve L x = z for each SNP: add trailing dim for batch solve
    xRinvs = np.linalg.solve(Ls, Z_score[:, :, None])[:, :, 0]  # (M, P)
    logSqrtDetSigmas = np.sum(np.log(np.diagonal(Ls, axis1=1, axis2=2)), axis=1)  # (M,)
    quadforms = np.sum(xRinvs**2, axis=1)  # (M,)
    jointProbs = np.exp(-0.5 * quadforms - logSqrtDetSigmas - P * np.log(2 * np.pi) / 2)
    return jointProbs

def flatten_out_omega(omega_est):
    """Flatten Cholesky for optimization."""
    P_c = len(omega_est)
    x_chol = np.linalg.cholesky(omega_est)
    lowTr_ind = np.tril_indices(P_c)
    x_chol_trf = np.zeros((P_c, P_c))
    for i in range(P_c):
        for j in range(i):
            x_chol_trf[i,j] = x_chol[i,j]/np.sqrt(x_chol[i,i]*x_chol[j,j])
    x_chol_trf[np.diag_indices(P_c)] = np.log(np.diag(x_chol))
    return tuple(x_chol_trf[lowTr_ind])

def rebuild_omega(chol_elems, s=None):
    """Rebuild Omega from flattened Cholesky."""
    if s is None:
        P = int((-1 + np.sqrt(1.+ 8.*len(chol_elems)))/2.)
        s = np.ones(P, dtype=bool)
        P_c = P
    else:
        P_c = int(np.sum(s))
        P = s.shape[1] if s.ndim == 2 else len(s)
    cholL = np.zeros((P_c, P_c))
    cholL[np.tril_indices(P_c)] = np.array(chol_elems)
    cholL[np.diag_indices(P_c)] = np.exp(np.diag(cholL))
    for i in range(P_c):
        for j in range(i):
            cholL[i,j] = cholL[i,j]*np.sqrt(cholL[i,i]*cholL[j,j])
    omega_c = np.dot(cholL, cholL.T)
    omega = np.zeros((P, P))
    s_caus_ind = np.argwhere(np.outer(s, s))
    omega[(s_caus_ind[:,0], s_caus_ind[:,1])] = omega_c.flatten()
    return omega

def _omega_neglogL(x, Zs, N_mats, sigma_LD):
    omega_it = rebuild_omega(x)
    joint_prob = jointEffect_probability(Zs, omega_it, sigma_LD, N_mats)
    return -np.sum(np.log(joint_prob))

def numerical_omega(Zs, Ns, sigma_LD, omega_start, tol=1e-6):
    """Numerical MLE estimation of Omega."""
    M, P = Zs.shape
    N_mats = np.sqrt(np.einsum('mp, mq -> mpq', Ns, Ns))
    solver_options = dict()
    solver_options['fatol'] = 1.0e-8
    solver_options['xatol'] = tol
    solver_options['disp'] = False
    solver_options['maxiter'] = P*(P+1)*500
    x_start = flatten_out_omega(omega_start)
    opt_results = minimize(_omega_neglogL, x_start, args=(Zs, N_mats, sigma_LD),
                           method='Nelder-Mead', options=solver_options)
    return rebuild_omega(opt_results.x), opt_results


# ============================================================
# Generate test scenarios
# ============================================================

np.set_printoptions(precision=15)

results = {}

# --- Scenario 1: 2 traits, 100 SNPs, simple case ---
np.random.seed(42)
M1, P1 = 100, 2
# Create Z-scores with some signal
true_omega = np.array([[0.05, 0.03], [0.03, 0.04]])
true_sigma = np.array([[1.0, 0.5], [0.5, 1.0]])
Ns1 = np.column_stack([
    np.random.choice([1000, 2000, 3000], M1).astype(float),
    np.random.choice([1500, 2500], M1).astype(float),
])
# Simulate Z-scores
N_mats1 = np.sqrt(np.einsum('mp,mq->mpq', Ns1, Ns1))
Zs1 = np.zeros((M1, P1))
for m in range(M1):
    cov = N_mats1[m] * true_omega + true_sigma
    Zs1[m] = np.random.multivariate_normal(np.zeros(P1), cov)

# GMM omega
omega_gmm = gmm_omega(Zs1, Ns1, true_sigma)
omega_gmm_adj = _posDef_adjustment(omega_gmm.copy())

# MTAG analysis with the true sigma and GMM omega
mtag_betas, mtag_se, mtag_factor = mtag_analysis(Zs1, Ns1, omega_gmm_adj, true_sigma)

results['scenario1'] = {
    'M': M1, 'P': P1,
    'Zs': Zs1.tolist(),
    'Ns': Ns1.tolist(),
    'sigma_LD': true_sigma.tolist(),
    'omega_gmm': omega_gmm.tolist(),
    'omega_gmm_adj': omega_gmm_adj.tolist(),
    'mtag_betas': mtag_betas.tolist(),
    'mtag_se': mtag_se.tolist(),
    'mtag_factor': mtag_factor.tolist(),
}

# --- Scenario 2: 3 traits, 200 SNPs ---
np.random.seed(123)
M2, P2 = 200, 3
true_omega2 = np.array([
    [0.04, 0.02, 0.01],
    [0.02, 0.05, 0.03],
    [0.01, 0.03, 0.03],
])
true_sigma2 = np.array([
    [1.0, 0.3, 0.2],
    [0.3, 1.0, 0.4],
    [0.2, 0.4, 1.0],
])
Ns2 = np.column_stack([
    np.full(M2, 1000.0),
    np.full(M2, 2000.0),
    np.random.choice([1500.0, 3000.0], M2),
])
N_mats2 = np.sqrt(np.einsum('mp,mq->mpq', Ns2, Ns2))
Zs2 = np.zeros((M2, P2))
for m in range(M2):
    cov = N_mats2[m] * true_omega2 + true_sigma2
    Zs2[m] = np.random.multivariate_normal(np.zeros(P2), cov)

omega_gmm2 = gmm_omega(Zs2, Ns2, true_sigma2)
omega_gmm_adj2 = _posDef_adjustment(omega_gmm2.copy())

mtag_betas2, mtag_se2, mtag_factor2 = mtag_analysis(Zs2, Ns2, omega_gmm_adj2, true_sigma2)

results['scenario2'] = {
    'M': M2, 'P': P2,
    'Zs': Zs2.tolist(),
    'Ns': Ns2.tolist(),
    'sigma_LD': true_sigma2.tolist(),
    'omega_gmm': omega_gmm2.tolist(),
    'omega_gmm_adj': omega_gmm_adj2.tolist(),
    'mtag_betas': mtag_betas2.tolist(),
    'mtag_se': mtag_se2.tolist(),
    'mtag_factor': mtag_factor2.tolist(),
}

# --- Scenario 3: 2 traits, 50 SNPs, numerical omega ---
np.random.seed(456)
M3, P3 = 50, 2
true_omega3 = np.array([[0.1, 0.05], [0.05, 0.08]])
true_sigma3 = np.array([[1.0, 0.3], [0.3, 1.0]])
Ns3 = np.full((M3, P3), 1000.0)
N_mats3 = np.sqrt(np.einsum('mp,mq->mpq', Ns3, Ns3))
Zs3 = np.zeros((M3, P3))
for m in range(M3):
    cov = N_mats3[m] * true_omega3 + true_sigma3
    Zs3[m] = np.random.multivariate_normal(np.zeros(P3), cov)

# GMM starting point (diagonal only)
gmm3 = gmm_omega(Zs3, Ns3, true_sigma3)
omega_start3 = np.zeros((P3, P3))
np.fill_diagonal(omega_start3, np.diag(gmm3))

omega_num3, opt3 = numerical_omega(Zs3, Ns3, true_sigma3, omega_start3)

# MTAG with numerical omega
mtag_betas3, mtag_se3, mtag_factor3 = mtag_analysis(Zs3, Ns3, omega_num3, true_sigma3)

results['scenario3_numerical'] = {
    'M': M3, 'P': P3,
    'Zs': Zs3.tolist(),
    'Ns': Ns3.tolist(),
    'sigma_LD': true_sigma3.tolist(),
    'omega_start': omega_start3.tolist(),
    'omega_numerical': omega_num3.tolist(),
    'mtag_betas': mtag_betas3.tolist(),
    'mtag_se': mtag_se3.tolist(),
    'mtag_factor': mtag_factor3.tolist(),
}

# --- Scenario 4: posdef adjustment ---
# A non-PSD matrix that needs adjustment
mat_nonpsd = np.array([
    [1.0, 0.8, 0.7],
    [0.8, 1.0, 0.8],
    [0.7, 0.8, 1.0],
])
# Make it non-PSD by scaling off-diagonals
mat_bad = mat_nonpsd.copy()
mat_bad[0, 1] = 1.5
mat_bad[1, 0] = 1.5
mat_adj = _posDef_adjustment(mat_bad.copy())

results['scenario4_posdef'] = {
    'input': mat_bad.tolist(),
    'adjusted': mat_adj.tolist(),
    'corr': cov2corr(mat_adj).tolist(),
}

# Save results
with open('golden_outputs.json', 'w') as f:
    json.dump(results, f, indent=2)

print(f"Generated golden outputs for {len(results)} scenarios")
for name, data in results.items():
    print(f"  {name}: {data['M']} SNPs × {data['P']} traits")
