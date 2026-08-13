//! Burn backend type aliases.
//!
//! `Backend` = autodiff-enabled (training).
//! `B` = plain inference backend (no graph).

use burn_autodiff::Autodiff;
use burn_ndarray::{NdArray, NdArrayDevice};

/// Training backend: Autodiff over NdArray (CPU, f32).
pub type Backend = Autodiff<NdArray>;

/// Inference backend: NdArray without autodiff.
pub type B = NdArray;

/// Default CPU device.
pub fn device() -> NdArrayDevice {
    NdArrayDevice::Cpu
}
