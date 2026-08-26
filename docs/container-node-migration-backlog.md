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
| LAVA univ/bivar/pcor/multireg | unified `lava_container` implemented; panel_id parameterised; all four official APIs have k3s baselines against the tutorial panel | official R `LAVA` 0.1.5 at commit `4738b09` | `lava.ref.1000g_test` tutorial package (univ/bivar/pcor/multireg passed); `lava.ref.ukb_eur` v1.1 published (47 files, 15 GiB) | switch the binding to `lava.ref.ukb_eur` once we add a chr1-22+chrX sumstats fixture, then unregister native LAVA Rust nodes |
| MiXeR fit1/fit2 | `mixer_fit1_container` and `mixer_fit2_container` implemented and k3s-tested | official `precimed/gsa-mixer` 2.2.1 at commit `ea2a445` | `mixer.g1000_eur` v2.2.1 catalog package | promote the local image to an internal registry and pin it by digest |

| COLOC `coloc.abf` | `coloc_abf_container` implemented; unit tests passing; wrapper unit tests build the official contract | official R `coloc` 5.2.3 (CRAN Archive) | no reference panel | pin image digest by promoting the local tag to an internal registry and re-running the k3s e2e baseline |

### Verified local provenance

| Runtime | Source pin | Local image manifest digest | Notes |
|---|---|---|---|
| LDSC 3.0.1 | CBIIT LDSC commit `6c67395` | `sha256:97145ca58cbb0c1c5d0689e91e79a8c256a52cf2dfd1cc936157cac993b1e410` | Verified h² and rg k3s baselines |
| MAGMA 1.10 | Official static binary SHA-256 `77fa456229963c9fb99e9f55bff61b68302f2f989d588ffd11cdd101fc6edab4` | `sha256:6642a6b0e55f727e69c121bb620b6dc7471747152ff0ada3c7be39b631cc875b` | Verified annotation k3s baseline |
| MRPRESSO 1.0.0 | Official commit `3e3c92d7eda6dce0d1d66077373ec0f7ff4f7e87` | `sha256:abf935a2fa679e67d50fad871369d758861551d039ce952cdfca2ea4d3f8fd75` | Verified official SummaryStats k3s baseline |
| MVMR 0.4.8 | Official commit `bceaa38088d093a5d30c713afb016e7fbc7ed2be` | `sha256:ed540641b99017623f002a05a567da608a8d17d78b31d01119d068c87e66ffa2` | Verified official LDL/HDL-to-SBP k3s baseline |
| LAVA 0.1.5 | Official tag `v0.1.5b`, commit `4738b097bf929ec8af40225c196c57d46d3d8a22` | `sha256:01f87cdedb1c7d7f9ea46bd6acc59d0336c7042769934a04c1ff5e3d11249216` | License is all rights reserved; tutorial panel digest `sha256:e4ed2e0bbb958be41dda3603a80d5a92dac3c950467dd583ce8d4980a57c9459`; official tutorial univ k3s baseline verified |
| UKB LAVA binary LD reference v1.1 (panel) | published catalog package | official R `LAVA` 0.1.5 + UKB binary LD reference at upstream commit `4738b097bf929ec8af40225c196c57d46d3d8a22` | `lava.ref.ukb_eur` v1.1 panel package digest `sha256:e5a47ee791b93aa90aec1896f5b8d6cf5a169da6335f0f7a22fcd13011a87168` (47 files, 15 GiB) | production panel published; switch `lava_container` binding from tutorial panel to this package once bivar/pcor/multireg k3s baselines pass |
| MiXeR 2.2.1 | Official commit `ea2a445912f83e5767d67372b6075912ed5655d8` |
| coloc 5.2.3 | CRAN Archive `coloc_5.2.3.tar.gz` (Wallace, GPL-3.0-or-later) | not yet pinned — local tag `localhost/atc/coloc:5.2.3` | unit-test baseline verified; k3s e2e deferred until cluster image digest is recorded |
 `sha256:66159cf0b14397b667483f08ad953d98d88718d94ee1cec0db6475cd68e4450a` | Image ID `95f3275fc917adafcaa9d578a244eafd3737ec06eecab7a6d22df7ec4ba0523c`; panel digest `sha256:a3de3339288985120eeb66d9e8d21fa157b88798504a18d02be14319a17171c4`; official chr21-22 fit1 and fit2 baselines verified |

The local images are currently referenced by tag for k3s development. Before
cross-node production use, push them to an internal registry and update wrapper
constants to immutable registry digests.

## Wave 1: source-backed methods needing clean images

| Area | Official runtime | Reference data | Main blocker |
|---|---|---|---|
| sLDSC | LDSC 3.0.1 CLI | baselineLD v2.2 ref-LD, w-LD, and `M_5_50` files | package the multi-annotation panel and preserve official `.results` output |
| GenomicSEM munge/LDSC | GenomicSEM R 0.0.5 at commit `399200d` plus official LDSC | native HM3, LD, WLD, and M files in GenomicSEM naming | build pinned R image and preserve the official LDSC result object |
| SuSiE-RSS | `susieR` at an exact 0.16.x commit | locus-aligned full LD correlation matrix | resolve 0.16.1 source versus 0.16.6 golden provenance; define object serialization |
| TwoSampleMR | official R package 0.7.9 | offline 1000G PLINK clumping reference for reproducibility | pin dependency graph and generate an offline clumping + full `mr()` baseline |

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
