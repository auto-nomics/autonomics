# GCTA fixture data

`chr22.ma` is the official GCTA mBAT manual sample summary table. It uses the
GCTA-COJO `.ma` layout and contains 219 chromosome 22 variants (199 of which
match the cataloged 1000G EUR Phase3 PLINK panel).

`chr22.fastGWA` is a deterministic fastGWA-layout projection of that fixture:
CHR, SNP and POS were joined from the cataloged 1000G EUR chromosome 22 BIM,
while A1, A2, N, AF1, BETA, SE and P came from the official sample. The ACAT-V
integration test uses `max_maf=0.05` and `min_mac=1` so the fixture contains a
nonempty qualified gene set.

Source: https://yanglab.westlake.edu.cn/software/gcta/res/mBAT_cpp_manual_sample.zip
