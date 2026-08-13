//! Build script for `grf-sys`.
//!
//! Drives compilation of the vendored GRF C++ core at `vendor/grf/core` and
//! emits cargo's linker directives so downstream crates pick up the resulting
//! static library (`libgrf_core.a`) plus pthread / stdc++.
//!
//! Strategy:
//! - We do NOT modify GRF's own `CMakeLists.txt`. Instead, we use `cmake`'s
//!   `build` API directly and configure a fresh out-of-tree build tree.
//! - To avoid pulling GRF's `main()` binary (`grf` executable in CMakeLists)
//!   into the static archive, we list every `.cpp` under `core/src/**` and
//!   hand them to `cc::Build` for a SECOND compilation pass that produces
//!   `libgrf_core.a`. This sidesteps the executable-only default and gives us
//!   full visibility into flags.
//! - The C ABI wrapper (`wrapper/grf_shim.cpp`) is compiled as a *separate*
//!   static library `libgrf_shim.a` and is what downstream Rust code actually
//!   links against. `libgrf_core.a` is a `links = "grf_core"` dependency that
//!   gets pulled in transitively.

use std::path::{Path, PathBuf};

/// Vendored GRF source, living inside this crate under `vendor/grf/`.
const GRF_CORE_DIR: &str = "vendor/grf/core";
const GRF_THIRD_PARTY: &str = "vendor/grf/core/third_party";

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let core_dir = manifest_dir.join(GRF_CORE_DIR);
    let third_party = manifest_dir.join(GRF_THIRD_PARTY);

    assert!(
        core_dir.exists(),
        "vendored GRF core not found at {}",
        core_dir.display()
    );

    // 1) Configure + build the vendored CMake project as a STATIC library we
    //    own. We sidestep GRF's executable target by directly invoking CMake
    //    and then re-archiving the .o files via `ar` ourselves. But the
    //    simplest robust approach: let CMake build the `grf` executable, then
    //    we extract its object files. Even simpler: do NOT use CMake at all
    //    and instead compile the core sources ourselves with `cc::Build` --
    //    they're plain C++17 + Eigen + pthread, with no codegen.
    //
    //    We choose the latter (cc::Build direct) -- this avoids the executable
    //    target entirely, gives us deterministic flag handling, and matches
    //    how ldsc-sys builds its C++ deps.

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let grf_lib_dir = out_dir.join("grf_lib");
    std::fs::create_dir_all(&grf_lib_dir).unwrap();

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .opt_level(2)
        .warnings(false) // GRF core generates a few deprecation warnings on newer GCC; suppress.
        .extra_warnings(false)
        .include(&core_dir.join("src"))
        .include(&core_dir.join("third_party"))
        .include(&core_dir.join("third_party").join("Eigen"))
        .flag("-Wno-deprecated-declarations")
        .flag("-Wno-unused-but-set-variable")
        .flag("-pthread")
        // The core CMakeLists defines these; mirror them.
        .define("NDEBUG", None);

    // Walk core/src/**/*.cpp recursively.
    let sources = collect_cpp_sources(&core_dir.join("src"));
    for src in &sources {
        println!("cargo:rerun-if-changed={}", src.display());
        build.file(src);
    }
    // cc::Build::compile returns (); it manages its own out-dir under
    // cargo's OUT_DIR. We don't need the path; the staticlib is at the
    // location cc chose.
    build.compile("grf_core");

    // 2) Compile the C ABI wrapper as a separate staticlib. It links against
    //    the core staticlib above.
    let shim = manifest_dir.join("wrapper").join("grf_shim.cpp");
    println!("cargo:rerun-if-changed={}", shim.display());
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .opt_level(2)
        .warnings(false)
        .include(&core_dir.join("src"))
        .include(&core_dir.join("third_party"))
        .include(&core_dir.join("third_party").join("Eigen"))
        .flag("-pthread")
        .flag("-Wno-deprecated-declarations")
        .file(&shim)
        .compile("grf_shim");

    // 3) Re-run whenever any tracked file changes.
    println!("cargo:rerun-if-changed={}", core_dir.join("src").display());
    println!(
        "cargo:rerun-if-changed={}",
        core_dir.join("third_party").display()
    );
    println!("cargo:rerun-if-changed=wrapper/grf_shim.cpp");
    println!("cargo:rerun-if-changed=build.rs");

    // 4) Emit linker directives so crates depending on `grf-sys` link against
    //    the staticlibs and pthread + stdc++.
    println!("cargo:rustc-link-lib=static=grf_shim");
    println!("cargo:rustc-link-lib=static=grf_core");
    println!("cargo:rustc-link-lib=stdc++");
    println!("cargo:rustc-link-lib=pthread");

    // Silence unused-var warning when not running cmake path.
    let _ = (GRF_THIRD_PARTY, third_party, out_dir, grf_lib_dir);
}

fn collect_cpp_sources(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = match std::fs::read_dir(&d) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|s| s.to_str()) == Some("cpp") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}
