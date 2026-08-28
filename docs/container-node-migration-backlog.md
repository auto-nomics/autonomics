# Official Container Migration Backlog

This backlog applies the non-negotiable policy in
[Container Node Migration Workflow](container-node-migration.md): a replacement
container node must execute an official original implementation, never a Rust
port. Existing native nodes remain registered as transitional fallbacks until
the official replacement passes a reproducible end-to-end baseline.

## Wave 0: completed or immediately ready

| Area | Replacement status | Official runtime | Reference data | Remaining gate |
|---|---|---|---|---|
| LDSC h² | `ldsc_h2_container` implemented and tested through k3s | official CBIIT LDSC continuation 3.0.1 | native EUR ref-LD and w-LD catalog packages | pin the image by digest and record clean source commit in provenance metadata |
| MAGMA annotate | `magma_annotate_container` implemented and k3s-tested | official MAGMA v1.10 static executable | `magma.gene_loc.ncbi37_3` catalog package | pin image digest and retain official annotation fixture |
| MAGMA gene/set/meta | not yet wrapped | official MAGMA v1.10 | full population LD bundles and official `.genes.raw` fixtures | publish large population panels; generate meta baseline |
| LDSC rg | `ldsc_rg_container` implemented and k3s-tested | official LDSC 3.0.1 image already built | native EUR ref-LD and w-LD packages | pin image digest and retain the official baseline fixture |
| MRPRESSO | `mrpresso_container` implemented and k3s-tested | official R `MRPRESSO` 1.0 at commit `3e3c92d` | no reference panel | pin image digest and retain official SummaryStats baseline |
| MVMR | `mvmr_container` implemented and k3s-tested | official R `MVMR` 0.4.8 at commit `bceaa38` | no reference panel | pin dependency versions and promote the image to an internal registry |
| LAVA univ/bivar/pcor/multireg | `lava_container` is the registered runtime; panel_id parameterised; native `lava_locus/lava_univ/lava_bivar/lava_pcor/lava_multireg` Rust nodes unregistered | official R `LAVA` 0.1.5 at commit `4738b09` | default binding `lava.ref.ukb_eur` v1.1; tutorial `lava.ref.1000g_test` remains available for regression | UKB univ k3s baseline verified; bivar/pcor/multireg can reuse the UKB panel as soon as a chr1-22+X sumstats fixture is available |
| MiXeR fit1/fit2 | `mixer_fit1_container` and `mixer_fit2_container` implemented and k3s-tested | official `precimed/gsa-mixer` 2.2.1 at commit `ea2a445` | `mixer.g1000_eur` v2.2.1 catalog package | promote the local image to an internal registry and pin it by digest |
| PLINK2 offline clump | `plink2_clump_container` implemented and k3s-tested | official PLINK2 v2.0.0-a.6.26 binary | `plink.ref.1000g_eur.binary` v1 catalog package | pin image digest and promote the local image to an internal registry |
| SuSiE-RSS | `susie_rss_container` is the registered runtime; native `susie_rss` Rust node unregistered | official `susieR` 0.16.6 at commit `ef213fe` | `mixer.g1000_eur` signed-LD panel | add TLS/authentication before extending the registry beyond the single-node baseline |
| SMR/HEIDI | `smr_heidi_container` implemented and k3s-tested | official SMR 1.4.2 Linux executable | default `smr.eqtl.westra_hg19` or optional `smr.eqtl.eqtlgen_hg19` BESD package plus `plink.ref.1000g_eur.binary` | none for the Westra chr22 contract; verify the eQTLGen chr22 contract |
| HyPrColoc | `hyprcoloc_container` implemented and locally baselined | official R `hyprcoloc` 0.0.2 at commit `0348bbd` | none for the basic beta/SE contract | import the local image into k3s, run the official 10-trait k3s baseline, then pin the image digest |
| GCTA summary statistics | `gcta_cojo_select_container`, `gcta_sblup_container`, `gcta_fastbat_container`, and `gcta_acat_container` implemented and k3s-tested | official GCTA 1.95.3 Linux executable | `plink.ref.1000g_eur.binary` plus `gcta.gene_list.hg19` | none for the committed chr22 summary-statistics contract |

| COLOC `coloc.abf` | `coloc_abf_container` implemented; unit tests passing; wrapper unit tests build the official contract | official R `coloc` 5.2.3 (CRAN Archive) | no reference panel | pin image digest by promoting the local tag to an internal registry and re-running the k3s e2e baseline |

### Verified local provenance

| Runtime | Source pin | Local image manifest digest | Notes |
|---|---|---|---|
| LDSC 3.0.1 | CBIIT LDSC commit `6c67395` | `sha256:97145ca58cbb0c1c5d0689e91e79a8c256a52cf2dfd1cc936157cac993b1e410` | Verified h² and rg k3s baselines |
| MAGMA 1.10 | Official static binary SHA-256 `77fa456229963c9fb99e9f55bff61b68302f2f989d588ffd11cdd101fc6edab4` | `sha256:6642a6b0e55f727e69c121bb620b6dc7471747152ff0ada3c7be39b631cc875b` | Verified annotation k3s baseline |
| MRPRESSO 1.0.0 | Official commit `3e3c92d7eda6dce0d1d66077373ec0f7ff4f7e87` | `sha256:abf935a2fa679e67d50fad871369d758861551d039ce952cdfca2ea4d3f8fd75` | Verified official SummaryStats k3s baseline |
| MVMR 0.4.8 | Official commit `bceaa38088d093a5d30c713afb016e7fbc7ed2be` | `sha256:ed540641b99017623f002a05a567da608a8d17d78b31d01119d068c87e66ffa2` | Verified official LDL/HDL-to-SBP k3s baseline |
| LAVA 0.1.5 | Official tag `v0.1.5b`, commit `4738b097bf929ec8af40225c196c57d46d3d8a22` | `sha256:01f87cdedb1c7d7f9ea46bd6acc59d0336c7042769934a04c1ff5e3d11249216` | License is all rights reserved; tutorial panel digest `sha256:e4ed2e0bbb958be41dda3603a80d5a92dac3c950467dd583ce8d4980a57c9459`; official tutorial univ k3s baseline verified |
| UKB LAVA binary LD reference v1.1 (panel) | published catalog package | official R `LAVA` 0.1.5 + UKB binary LD reference at upstream commit `4738b097bf929ec8af40225c196c57d46d3d8a22` | `lava.ref.ukb_eur` v1.1 panel package digest `sha256:e5a47ee791b93aa90aec1896f5b8d6cf5a169da6335f0f7a22fcd13011a87168` (47 files, 15 GiB) | `lava_container` now defaults to this panel; real k3s univ baseline passed against chr1 tutorial sumstats (`75702 SNPs shared`) |
| PLINK2 2.0.0-a.6.26 | Official tag `v2.0.0-a.6.26`, commit `faed32c9`, asset SHA-256 `f578a450af382d7dd6665aecf0ca1d280971c2b3d5bb5556efbf9266c4c8da0f` | `sha256:4dbbfbd2b1deabebfa36691516acf7b48a54c5e9b0ae50e62943b64e907ad4e0` | Image ID `a1861e294cf6`; panel digest `sha256:80597da4137e90c3c312c9fa37a4dae9d29f18b7a8b12576ae502941bb1a558d`; real chr22 clump baseline verified (10 fixture variants -> 2 index variants) |
| susieR 0.16.6 | Official tag `0.16.6`, commit `ef213feed2cb82419677661a8c986e1504df2c73` | `sha256:8a72a443461add5c94c4907f9a1d6106b989850e93217a95587d9095542febe8` | Published to `192.168.10.24:30500/atc/susie`; image ID `7bb2839fe57e`; panel digest `sha256:a3de3339288985120eeb66d9e8d21fa157b88798504a18d02be14319a17171c4`; real chr21 baseline verified (50 fixture variants -> 4 credible sets) |
| MiXeR 2.2.1 | Official commit `ea2a445912f83e5767d67372b6075912ed5655d8` |
| coloc 5.2.3 | CRAN Archive `coloc_5.2.3.tar.gz` (Wallace, GPL-3.0-or-later) | not yet pinned — local tag `localhost/atc/coloc:5.2.3` | unit-test baseline verified; k3s e2e deferred until cluster image digest is recorded |
| SMR 1.4.2 | Official Linux zip SHA-256 `c01ef6a4c5d03c7d504e24b2ccf40ab4da56eb127295c7671c867048020f893c` (executable MIT; source GPL-2.0) | `192.168.10.24:30500/atc/smr@sha256:40c0db3c71eda506913c376ab939027fff8ce8eb55c262fd1e5da2fe4c351b6d` | Image config ID `06f986ca1115`; Westra panel digest `sha256:cf3ff9ae5d6e4d2f6cd12ca450fd3d415c9c5ef3003eb52738e11ca8e4343f1b`; official chr22 k3s baseline verified (5 probes; `ILMN_1765304/PSITPTE22` p_SMR `2.684279e-06`, HEIDI 7 SNPs) |
| HyPrColoc 0.0.2 | Official tag `v0.0.2`, commit `0348bbdd977be731d82e4efda115a9bed63cd44f`; source archive SHA-256 `a66f478b62453b5ecdc7eef16eda82d131c7dcafa65fbb80b6e536bc978797e3` | not yet pinned — local tag `localhost/atc/hyprcoloc:0.0.2`, image ID `ca1f1812dfb8900057997f8d432c43c182cae3d329beeaa71ff839ff20e5dabb` | Official 10-trait fixture is generated from `data/test.RData` (SHA-256 `caa7a96190c4a292579393926dcac92c3dbf78840b7948c06107486c740b45f5`); local baseline reproduces T1-T5/T6-T8/T9-T10 and candidates `rs11591147`, `rs12117612`, `rs7524677`; k3s image import pending |
| GCTA 1.95.3 | Official Linux zip SHA-256 `441e01715bc12dabb083fe76372432139dff9bb2c073d21d218d15e92580e807` (executable MIT; source GPL-3.0-or-later) | `192.168.10.24:30500/atc/gcta@sha256:4cbf8c91376f7b314eebf1dfa02ad44028575991cd3d4c4e81b5324942bec20b` | Image ID `bf00e6467deb`; PLINK panel digest `sha256:80597da4137e90c3c312c9fa37a4dae9d29f18b7a8b12576ae502941bb1a558d`; hg19 gene-list digest `sha256:72d0baec00cd512482bf510f76da6264eed67cb71f57654aff9553e1ed108db8`; chr22 baselines verified (COJO 3 signals, SBLUP 199 effects, fastBAT 4 genes, ACAT-V 1 gene) |
 `sha256:66159cf0b14397b667483f08ad953d98d88718d94ee1cec0db6475cd68e4450a` | Image ID `95f3275fc917adafcaa9d578a244eafd3737ec06eecab7a6d22df7ec4ba0523c`; panel digest `sha256:a3de3339288985120eeb66d9e8d21fa157b88798504a18d02be14319a17171c4`; official chr21-22 fit1 and fit2 baselines verified |

The SuSiE-RSS image is published to the single-node k3s registry and referenced
by immutable digest. The other local images are still referenced by tag for
k3s development; promote them to the same registry and pin their digests as
each migration is completed.

## Wave 1: source-backed methods needing clean images

| Area | Official runtime | Reference data | Main blocker |
|---|---|---|---|
| sLDSC | LDSC 3.0.1 CLI | baselineLD v2.2 ref-LD, w-LD, and `M_5_50` files | package the multi-annotation panel and preserve official `.results` output |
| GenomicSEM munge/LDSC | GenomicSEM R 0.0.5 at commit `399200d` plus official LDSC | native HM3, LD, WLD, and M files in GenomicSEM naming | build pinned R image and preserve the official LDSC result object |
| TwoSampleMR | official R package 0.7.9 | offline 1000G PLINK clumping reference for reproducibility | integrate the verified `plink2_clump_container` into the official TwoSampleMR R image; pin dependency graph and generate a full `mr()` baseline |

## Wave 2: blocked on official data or provenance

| Area | Official runtime | Missing prerequisite |
|---|---|---|
| HDL-L and whole-genome scan | HDL R 1.4.3 at commit `e6b055d` | official HDL LD SVD package and official piece-coordinate mapping |
| MRlap | MRlap R 0.0.3.3 plus compatible GenomicSEM | exact dependency pinning and full official example baseline |
| LCV | official LCV scripts at commit `39950a8` | license/provenance approval and official native LD-score input baseline |
| MTAG | official MTAG 1.0.8 source | reacquire and pin exact source/release; current provenance is insufficient |
| CPASSOC | official Zhu & Feng R implementation | reacquire exact source and API; current local reference is absent |

## Guardrails

- Never package a local Rust port as the official runtime.
- Do not use dirty sibling checkouts as image build contexts; fetch clean
  archives at pinned commits/tags.
- Prefer immutable image digests in wrapper constants before calling a migration
  production-ready.
- Large reference panels stay in the catalog; images contain only tools and
  dependencies.
- Keep existing native factories registered until the official wrapper passes
  a k3s end-to-end baseline against an immutable fixture.
