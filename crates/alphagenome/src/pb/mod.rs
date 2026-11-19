//! Generated protobuf / gRPC code.
//!
//! Sources are the protos vendored under `proto/`, taken from
//! <https://github.com/google-deepmind/alphagenome> (Apache-2.0). The
//! generated files cross-reference each other through the package module
//! path (`super::super::v1main`), so the include nesting below must mirror
//! the proto package structure exactly.

pub mod google {
    pub mod gdm {
        pub mod gdmscience {
            pub mod alphagenome {
                pub mod atlas {
                    pub mod v1main {
                        include!(concat!(
                            env!("OUT_DIR"),
                            "/google.gdm.gdmscience.alphagenome.atlas.v1main.rs"
                        ));
                    }
                }
                pub mod v1main {
                    include!(concat!(
                        env!("OUT_DIR"),
                        "/google.gdm.gdmscience.alphagenome.v1main.rs"
                    ));
                }
            }
        }
    }
}

/// Atlas service types (`...alphagenome.atlas.v1main`).
pub use google::gdm::gdmscience::alphagenome::atlas::v1main as atlas;

/// DNA model types (`...alphagenome.v1main`).
pub use google::gdm::gdmscience::alphagenome::v1main as v1;
