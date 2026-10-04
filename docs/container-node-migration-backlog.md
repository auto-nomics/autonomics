# Official Container Migration Backlog

This backlog applies the non-negotiable policy in
[Container Node Migration Workflow](container-node-migration.md): a replacement
container node must execute an official original implementation, never a Rust
port. Existing native nodes remain registered as transitional fallbacks until
the official replacement passes a reproducible end-to-end baseline.

## Wave 0: completed or immediately ready

| Area | Replacement status | Official runtime | Reference data | Remaining gate |
|---|---|---|---|---|
| LDSC h² | `ldsc_h2_container` implemented and tested through the container backend; the native Rust h² node is removed | official CBIIT LDSC continuation 3.0.1 | native EUR ref-LD and w-LD catalog packages | pin the image by digest and record clean source commit in provenance metadata |
| MAGMA annotate | `magma_annotate_container` implemented and container-tested; native annotation implementation removed | official MAGMA v1.10 static executable | `wjixiang/catalog-magma-gene-loc-ncbi37-3` catalog package | pin image digest and retain official annotation fixture |
| MAGMA gene/set/meta | not yet wrapped | official MAGMA v1.10 | full population LD bundles and official `.genes.raw` fixtures | publish large population panels; generate meta baseline |
| LDSC rg | `ldsc_rg_container` implemented and container-tested; the native Rust rg node is removed | official LDSC 3.0.1 image already built | native EUR ref-LD and w-LD packages | pin image digest and retain the official baseline fixture |
| MRPRESSO | `mrpresso_container` implemented and container-tested; the native Rust crate and unregistered node are removed | official R `MRPRESSO` 1.0 at commit `3e3c92d` | no reference panel | pin image digest and retain official SummaryStats baseline |
| MVMR | `mvmr_container` implemented and container-tested; the native Rust crate and unregistered node are removed | official R `MVMR` 0.4.8 at commit `bceaa38` | no reference panel | pin dependency versions and promote the image to an internal registry |
| LAVA univ/bivar/pcor/multireg and scan | `lava_container` and `lava_scan_container` are the registered runtimes; native `lava_locus/lava_univ/lava_bivar/lava_pcor/lava_multireg` Rust nodes are unregistered | official R `LAVA` 0.1.5 at commit `4738b09` | both bind `wjixiang/catalog-lava-ref-ukb-eur` v1.1 by default; tutorial `wjixiang/catalog-lava-ref-1000g-test` remains available for regression | UKB univ container baseline verified; tutorial two-locus official `run.univ.bivar` Podman scan verified; run a production UKB scan baseline when a full loci/sumstats fixture is available |
| MiXeR fit1/fit2 | `mixer_fit1_container` and `mixer_fit2_container` implemented and Podman-tested; the unused native Rust crate is removed | official `precimed/gsa-mixer` 2.2.1 at commit `ea2a445` | `wjixiang/catalog-mixer-g1000-eur-rsid` v2.2.1-rsid1 | none; the GHCR image is pinned by digest |
| PLINK2 offline clump | `plink2_clump_container` implemented and container-tested | official PLINK2 v2.0.0-a.6.26 binary | `wjixiang/catalog-plink-ref-1000g-eur-binary` v1 catalog package | pin image digest and promote the local image to an internal registry |
| SuSiE-RSS | `susie_rss_container` is the registered runtime; the native Rust crate is removed | official `susieR` 0.16.6 at commit `ef213fe` | `wjixiang/catalog-mixer-g1000-eur` signed-LD panel | add TLS/authentication before extending the registry beyond the single-node baseline |
| SMR/HEIDI | `smr_heidi_container` implemented and container-tested | official SMR 1.4.2 Linux executable | default `wjixiang/catalog-smr-eqtl-westra-hg19` or optional `wjixiang/catalog-smr-eqtl-eqtlgen-hg19` BESD package plus `wjixiang/catalog-plink-ref-1000g-eur-binary` | none for the Westra chr22 contract; verify the eQTLGen chr22 contract |
| HyPrColoc | `hyprcoloc_container` implemented and locally baselined | official R `hyprcoloc` 0.0.2 at commit `0348bbd` | none for the basic beta/SE contract | run the official 10-trait Podman baseline, then pin the image digest |
| GCTA summary statistics | `gcta_cojo_select_container`, `gcta_sblup_container`, `gcta_fastbat_container`, and `gcta_acat_container` implemented and container-tested | official GCTA 1.95.3 Linux executable | `wjixiang/catalog-plink-ref-1000g-eur-binary` plus `wjixiang/catalog-gcta-gene-list-hg19` | none for the committed chr22 summary-statistics contract |
| MTAG | `mtag_container` implemented and Podman-tested | official MTAG 1.0.8 at commit `9e17f3c` | `wjixiang/catalog-mtag-ld-ref-1000g-eur-w-ld` catalog package | publish the image to the selected deployment registry when this contract leaves the local Podman workflow |
| HDL-L region and chromosome scan | `hdl_l_container` and the container-backed `hdl_l_scan` are the registered runtimes; the native Rust HDL-L crate and nodes are removed | official R `HDL` 1.4.3 at commit `e6b055d` | both bind published `wjixiang/catalog-hdl-ref-ukb-eur` v1.0 | none for the committed chr1/piece9 Podman baselines; a full all-chromosome production scan remains an operational runbook item |
| TwoSampleMR | `twosamplemr_container` implemented, GHCR-pinned, and covered by the official and catalog-backed Podman baselines; the native Rust `two_sample_mr` node is removed (`bio_crates/mr` remains as MRlap's estimator library) | official R `TwoSampleMR` 0.7.9 at commit `3d119f2` plus official PLINK2 2.0.0-a.6.26 | `wjixiang/catalog-plink-ref-1000g-eur-binary` | none for the committed contracts |

| Visualization | `visualization_container` is the registered runtime; the legacy host-R DataFrame `visualization` node is removed | pinned R 4.5.3 runtime with Arrow 23.0.1.2 and ggplot2 4.0.3 | no reference panel | none for the local File-to-File PNG baseline |
| COLOC `coloc.abf` | `coloc_abf_container` implemented; unit tests passing; wrapper unit tests build the official contract | official R `coloc` 5.2.3 (CRAN Archive) | no reference panel | pin image digest by promoting the local tag to an internal registry and re-running the Podman e2e baseline |
| Single-cell MatrixMarket preprocessing | `single_cell_preprocessor_container` implemented, GHCR-pinned, and covered by real Podman inspect/ingest plus production-scale MatrixMarket baselines | official Scanpy 1.11.3 with a locked wheel closure | no reference panel | none |

### Verified local provenance

| Runtime | Source pin | Local image manifest digest | Notes |
|---|---|---|---|
| LDSC 3.0.1 | CBIIT LDSC commit `6c67395` | `sha256:97145ca58cbb0c1c5d0689e91e79a8c256a52cf2dfd1cc936157cac993b1e410` | Verified h² and rg container baselines |
| MAGMA 1.10 | Official static binary SHA-256 `77fa456229963c9fb99e9f55bff61b68302f2f989d588ffd11cdd101fc6edab4` | `sha256:6642a6b0e55f727e69c121bb620b6dc7471747152ff0ada3c7be39b631cc875b` | Verified annotation container baseline |
| MRPRESSO 1.0.0 | Official commit `3e3c92d7eda6dce0d1d66077373ec0f7ff4f7e87` | `sha256:abf935a2fa679e67d50fad871369d758861551d039ce952cdfca2ea4d3f8fd75` | Verified official SummaryStats container baseline |
| MVMR 0.4.8 | Official commit `bceaa38088d093a5d30c713afb016e7fbc7ed2be` | `sha256:ed540641b99017623f002a05a567da608a8d17d78b31d01119d068c87e66ffa2` | Verified official LDL/HDL-to-SBP container baseline |
| LAVA 0.1.5 | Official tag `v0.1.5b`, commit `4738b097bf929ec8af40225c196c57d46d3d8a22` | `sha256:01f87cdedb1c7d7f9ea46bd6acc59d0336c7042769934a04c1ff5e3d11249216` | License is all rights reserved; tutorial panel digest `sha256:e4ed2e0bbb958be41dda3603a80d5a92dac3c950467dd583ce8d4980a57c9459`; official tutorial univ container baseline and two-locus `run.univ.bivar` Podman scan verified |
| UKB LAVA binary LD reference v1.1 (panel) | published catalog package | official R `LAVA` 0.1.5 + UKB binary LD reference at upstream commit `4738b097bf929ec8af40225c196c57d46d3d8a22` | `wjixiang/catalog-lava-ref-ukb-eur` v1.1 panel package digest `sha256:e5a47ee791b93aa90aec1896f5b8d6cf5a169da6335f0f7a22fcd13011a87168` (47 files, 15 GiB) | `lava_container` now defaults to this panel; real container univ baseline passed against chr1 tutorial sumstats (`75702 SNPs shared`) |
| PLINK2 2.0.0-a.6.26 | Official tag `v2.0.0-a.6.26`, commit `faed32c9`, asset SHA-256 `f578a450af382d7dd6665aecf0ca1d280971c2b3d5bb5556efbf9266c4c8da0f` | `sha256:4dbbfbd2b1deabebfa36691516acf7b48a54c5e9b0ae50e62943b64e907ad4e0` | Image ID `a1861e294cf6`; panel digest `sha256:80597da4137e90c3c312c9fa37a4dae9d29f18b7a8b12576ae502941bb1a558d`; real chr22 clump baseline verified (10 fixture variants -> 2 index variants) |
| susieR 0.16.6 | Official tag `0.16.6`, commit `ef213feed2cb82419677661a8c986e1504df2c73` | `sha256:8a72a443461add5c94c4907f9a1d6106b989850e93217a95587d9095542febe8` | Published to `192.168.10.24:30500/atc/susie`; image ID `7bb2839fe57e`; panel digest `sha256:a3de3339288985120eeb66d9e8d21fa157b88798504a18d02be14319a17171c4`; real chr21 baseline verified (50 fixture variants -> 4 credible sets) |
| MiXeR 2.2.1 | Official commit `ea2a445912f83e5767d67372b6075912ed5655d8` | `sha256:3bd67cccf298bd3c9af3d2b013dd7dfacde9ad13d51bc78b2f7f1315f01bebb7` | Published to `autonomics/mixer:2.2.1` in the configured GitHub Container Registry GHCR; image ID `95f3275fc917`; rsID panel digest `sha256:46a73e2e4faac1fc216e1d5918257546c87dae727a0f7a55cca39d87bdf4b1ca`; official chr21-22 fit1/fit2 baselines and production rsID smoke verified |
| coloc 5.2.3 | CRAN Archive `coloc_5.2.3.tar.gz` (Wallace, GPL-3.0-or-later) | not yet pinned — local tag `localhost/atc/coloc:5.2.3` | unit-test baseline verified; Podman e2e deferred until the image digest is recorded |
| SMR 1.4.2 | Official Linux zip SHA-256 `c01ef6a4c5d03c7d504e24b2ccf40ab4da56eb127295c7671c867048020f893c` (executable MIT; source GPL-2.0) | `192.168.10.24:30500/atc/smr@sha256:40c0db3c71eda506913c376ab939027fff8ce8eb55c262fd1e5da2fe4c351b6d` | Image config ID `06f986ca1115`; Westra panel digest `sha256:cf3ff9ae5d6e4d2f6cd12ca450fd3d415c9c5ef3003eb52738e11ca8e4343f1b`; official chr22 container baseline verified (5 probes; `ILMN_1765304/PSITPTE22` p_SMR `2.684279e-06`, HEIDI 7 SNPs) |
| HyPrColoc 0.0.2 | Official tag `v0.0.2`, commit `0348bbdd977be731d82e4efda115a9bed63cd44f`; source archive SHA-256 `a66f478b62453b5ecdc7eef16eda82d131c7dcafa65fbb80b6e536bc978797e3` | not yet pinned — local tag `localhost/atc/hyprcoloc:0.0.2`, image ID `ca1f1812dfb8900057997f8d432c43c182cae3d329beeaa71ff839ff20e5dabb` | Official 10-trait fixture is generated from `data/test.RData` (SHA-256 `caa7a96190c4a292579393926dcac92c3dbf78840b7948c06107486c740b45f5`); local baseline reproduces T1-T5/T6-T8/T9-T10 and candidates `rs11591147`, `rs12117612`, `rs7524677`; Podman image baseline pending |
| GCTA 1.95.3 | Official Linux zip SHA-256 `441e01715bc12dabb083fe76372432139dff9bb2c073d21d218d15e92580e807` (executable MIT; source GPL-3.0-or-later) | `192.168.10.24:30500/atc/gcta@sha256:4cbf8c91376f7b314eebf1dfa02ad44028575991cd3d4c4e81b5324942bec20b` | Image ID `bf00e6467deb`; PLINK panel digest `sha256:80597da4137e90c3c312c9fa37a4dae9d29f18b7a8b12576ae502941bb1a558d`; hg19 gene-list digest `sha256:72d0baec00cd512482bf510f76da6264eed67cb71f57654aff9553e1ed108db8`; chr22 baselines verified (COJO 3 signals, SBLUP 199 effects, fastBAT 4 genes, ACAT-V 1 gene) |
| MTAG 1.0.8 | JonJala MTAG commit `9e17f3cf1fbcf57b6bc466daefdc51fd0de3c5dc` | `sha256:28ac0a0a0ee741340390b7588bf8ba36e6b316d62adc0cab7fd4494f5dd6a90c` | Official Python 2.7 runtime; panel digest `sha256:4fb3a06b9b4a8acbe7b6766ffe946cd5524d9d4e1b6f64bf3660d12a5ca9438f`; rootless-Podman two-trait baseline verified with 199,647 output SNPs |
| HDL 1.4.3 | Official commit `e6b055d42fd9904c3e994a9b829626c7e5f8422b` | Published to `autonomics/hdl:1.4.3`, manifest digest `sha256:cbce3f3e4037b8c53c59275240f4449f2041de39aa95a4b5d9e588b9a75b18ea` | GHCR image reports HDL 1.4.3, data.table 1.18.4, and dplyr 1.2.1 and completed a local-panel chr1/piece3 region Podman smoke; published-panel chr1/piece9 region and scan numerical baselines were established with the source-pinned predecessor image; `wjixiang/catalog-hdl-ref-ukb-eur` v1.0 panel digest `sha256:411c7ae1db876ec3e17941367a74567175ca151f8f93dc6e5e1d06bb8f3a3f54` is not embedded in the image |
| TwoSampleMR 0.7.9 | Official tag `v0.7.9`, commit `3d119f20d6fc164b0c7f710f5590fee9580f2c7b`, source archive SHA-256 `6848c344c5eead601ff52e9a88b2c23c2b61b0ce4f848e331e7494b657be64e5` | `sha256:c270de9978906ee48cbba2ac484ba3df9e86dddc69efd908c6f908963114002a` | Published to `autonomics/twosamplemr:0.7.9`; official `test_commondata.RData` reproduces IVW `0.4459` and Egger `0.5025`; catalog-backed chr22 baseline reduces 10 candidates to 2 PLINK2 index instruments and completes official harmonisation plus `mr()` |
 `sha256:66159cf0b14397b667483f08ad953d98d88718d94ee1cec0db6475cd68e4450a` | Image ID `95f3275fc917adafcaa9d578a244eafd3737ec06eecab7a6d22df7ec4ba0523c`; panel digest `sha256:a3de3339288985120eeb66d9e8d21fa157b88798504a18d02be14319a17171c4`; official chr21-22 fit1 and fit2 baselines verified |
| Scanpy 1.11.3 | Official Scanpy wheel with the complete locked wheel closure in `containers/single-cell-preprocessor/requirements.txt` | `sha256:c34c26428d13804c2528bc734805c1ccfe909353604ac08da0ce9c68890e7e18` | Published to `autonomics/single-cell-preprocessor:0.1.0`; image config ID `1b10f7a01efd`; real Podman inspect/ingest baseline and a 20,000-gene x 50,000-cell / 20,000,000-nonzero gzip MatrixMarket inspect baseline verified |

The SuSiE-RSS image is published to the internal registry and referenced
by immutable digest. The other local images are still referenced by tag for
local development; promote them to the same registry and pin their digests as
each migration is completed.

## Wave 1: source-backed methods needing clean images

| Area | Official runtime | Reference data | Main blocker |
|---|---|---|---|
| sLDSC | LDSC 3.0.1 CLI | baselineLD v2.2 ref-LD, w-LD, and `M_5_50` files | package the multi-annotation panel and preserve official `.results` output |
| GenomicSEM munge/LDSC | GenomicSEM R 0.0.5 at commit `399200d` plus official LDSC | native HM3, LD, WLD, and M files in GenomicSEM naming | build pinned R image and preserve the official LDSC result object |

## Wave 2: blocked on official data or provenance

| Area | Official runtime | Missing prerequisite |
|---|---|---|
| MRlap | MRlap R 0.0.3.3 plus compatible GenomicSEM | exact dependency pinning and full official example baseline |
| LCV | official LCV scripts at commit `39950a8` | license/provenance approval and official native LD-score input baseline |
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
  a backend-appropriate reproducible end-to-end baseline against an immutable
  fixture. MTAG is currently scoped to the rootless-Podman workflow.
