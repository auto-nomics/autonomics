# Embedded manuscript compiler

The application embeds the official `tectonic = 0.17.0` Rust engine (including XeTeX, BibTeX and xdvipdfmx). Compilation starts the application's own executable with an internal worker flag before initializing the application runtime. It never invokes a system `tectonic`, `latex`, `xelatex` or `bibtex` command. `autonomics-manuscript-worker` is also available for independent testing/deployment.

The native build requires development packages for HarfBuzz, FreeType, Graphite2, ICU, fontconfig, libpng and zlib, plus a C/C++ toolchain and pkg-config. Supported execution target: Linux with Landlock ABI 3 or later (Linux 6.2+); unsupported platforms fail explicitly, rather than compiling without filesystem isolation.

The default bundle is pinned to `https://data1b.fullyjustified.net/tlextras-2022.0r0.tar`, the TeX Live support bundle for Tectonic format serial 33. The default bundle identity is checked against SHA-256 `6ffe055852f8faf66c0acbe1a7fb27f87b869a90bad1204f3bf4d9683f597c7c`; unexpected upstream bundle replacement fails explicitly. Tectonic fetches its indexed resources on demand and caches them by content digest; a full TeX distribution download is unnecessary. First use requires HTTPS access. Cache and bundle settings are administrator configuration, never manuscript request parameters:

- `AUTONOMICS_TECTONIC_CACHE`: persistent cache root (default `$HOME/.cache/autonomics/tectonic`).
- `AUTONOMICS_TECTONIC_BUNDLE`: pinned alternate HTTPS bundle or local bundle path (local directory, ZIP, or TTB supported by upstream).
- `AUTONOMICS_TECTONIC_TIMEOUT_SECS`: wall time limit, default 300 seconds, bounded to 5–600 seconds.
- `AUTONOMICS_TECTONIC_OFFLINE=1`: only cached resources; missing packages cause a visible build error.
- `AUTONOMICS_MANUSCRIPT_WORKER`: optional separately built worker executable; by default the current application executable is reused.

Provision and verify the bundle before offline deployment:

```sh
cargo build -p manuscript-engine --bin autonomics-manuscript-worker
cargo test -p manuscript-engine --test embedded -- --ignored --nocapture
```

Retain the configured cache directory across deployment. To provision packages beyond the article/BibTeX smoke test, compile the intended project/templates online before enabling offline mode. For fully offline installations, supply a complete local bundle. The cache cannot guarantee arbitrary packages which have never been fetched.

Two compile slots are shared across HTTP and agent calls. Every build gets a fresh project directory, cleared environment, 64 MiB source limit and 512-file limit. Filesystem access is enforced with Linux Landlock: only project, bundle cache and public TLS/DNS configuration are accessible. Shell escape and extra input paths are disabled; generated TeX outputs stay in memory, so project source cannot write arbitrary files. Workers are killed on timeout or cancellation. No unsafe fallback is provided if isolation is unavailable.

Artifact limits: 16 MiB PDF, 4 MiB SyncTeX, 1 MiB log, 256 diagnostics of at most 4 KiB each, and 96 MiB maximum serialized worker result. Exceeding the PDF limit fails the build visibly.
