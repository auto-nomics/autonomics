//! Local OCI image management contracts and backend implementations.

mod parser;
mod podman;
#[cfg(test)]
mod tests;
mod types;

pub use types::{
    EnsureImageResult, ImageInspect, ImageListOptions, ImageManager, ImagePullResult, ImageRecord,
    ImageRemoveOptions, ImageRemoveResult,
};
