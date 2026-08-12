//! Data conversion helpers between `dl::Tensor` (f64) and Burn tensors (f32).

use burn::tensor::{Tensor, TensorData};
use burn::module::Param;
use burn::nn::Linear;

use crate::backend::{B, Backend};
use burn_ndarray::NdArrayDevice;
use crate::configs::LayerWeights;
use crate::tensor::Tensor as DlTensor;

// ─── f64 Tensor → Burn tensors ─────────────────────────────────────────

/// Convert a `dl::Tensor` (f64, row-major) to a Burn 2-D autodiff tensor (f32).
pub fn f64_to_burn(data: &DlTensor, device: &NdArrayDevice) -> Tensor<Backend, 2> {
    let (nrows, ncols) = data.shape();
    let f32_data: Vec<f32> = data.as_flat().iter().map(|&v| v as f32).collect();
    Tensor::<Backend, 2>::from_data(TensorData::new(f32_data, [nrows, ncols]), device)
}

/// Convert a `dl::Tensor` to a Burn 2-D **inference** tensor (no autodiff).
pub fn f64_to_burn_infer(data: &DlTensor, device: &NdArrayDevice) -> Tensor<B, 2> {
    let (nrows, ncols) = data.shape();
    let f32_data: Vec<f32> = data.as_flat().iter().map(|&v| v as f32).collect();
    Tensor::<B, 2>::from_data(TensorData::new(f32_data, [nrows, ncols]), device)
}

/// Convert selected rows of a `dl::Tensor` to a Burn 2-D autodiff tensor.
pub fn rows_to_burn(
    data: &DlTensor,
    indices: &[usize],
    device: &NdArrayDevice,
) -> Tensor<Backend, 2> {
    let ncols = data.ncols();
    let mut f32_data = Vec::with_capacity(indices.len() * ncols);
    for &i in indices {
        for j in 0..ncols {
            f32_data.push(data.at(i, j) as f32);
        }
    }
    Tensor::<Backend, 2>::from_data(
        TensorData::new(f32_data, [indices.len(), ncols]),
        device,
    )
}

/// Convert selected rows' single column to a Burn 1-D autodiff tensor.
pub fn rows_col_to_burn_1d(
    data: &DlTensor,
    col: usize,
    indices: &[usize],
    device: &NdArrayDevice,
) -> Tensor<Backend, 1> {
    let f32_data: Vec<f32> = indices.iter().map(|&i| data.at(i, col) as f32).collect();
    Tensor::<Backend, 1>::from_data(TensorData::new(f32_data, [indices.len()]), device)
}

// ─── Burn tensor → f64 ─────────────────────────────────────────────────

/// Convert a Burn 2-D inference tensor to `dl::Tensor` (f64).
pub fn burn2d_to_tensor(tensor: Tensor<B, 2>) -> DlTensor {
    let shape = tensor.shape();
    let nrows = shape.dims[0];
    let ncols = shape.dims[1];
    let data = tensor.into_data();
    let slice = data.as_slice::<f32>().unwrap();
    let f64_data: Vec<f64> = slice.iter().map(|&v| v as f64).collect();
    DlTensor::from_rows(nrows, ncols, &f64_data)
}

/// Convert a Burn 2-D autodiff tensor to `dl::Tensor` (f64).
pub fn burn2d_autodiff_to_tensor(tensor: Tensor<Backend, 2>) -> DlTensor {
    let shape = tensor.shape();
    let nrows = shape.dims[0];
    let ncols = shape.dims[1];
    let data = tensor.into_data();
    let slice = data.as_slice::<f32>().unwrap();
    let f64_data: Vec<f64> = slice.iter().map(|&v| v as f64).collect();
    DlTensor::from_rows(nrows, ncols, &f64_data)
}

// ─── Weight extraction / reconstruction ────────────────────────────────

/// Extract weights from Burn Linear layers into serializable format.
pub fn extract_linear_weights<Bk: burn::tensor::backend::Backend>(
    layers: &[Linear<Bk>],
) -> Vec<LayerWeights> {
    layers
        .iter()
        .map(|l| {
            let w_val = l.weight.val();
            let w_shape = w_val.shape();
            // Burn Linear weight shape is [d_input, d_output].
            let in_features = w_shape.dims[0];
            let out_features = w_shape.dims[1];
            let w_data = w_val.into_data();
            let w_slice = w_data.as_slice::<f32>().unwrap();
            let weight: Vec<f64> = w_slice.iter().map(|&v| v as f64).collect();

            let bias: Vec<f64> = if let Some(b_ref) = &l.bias {
                let b_val = b_ref.val();
                let b_data = b_val.into_data();
                b_data
                    .as_slice::<f32>()
                    .unwrap()
                    .iter()
                    .map(|&v| v as f64)
                    .collect()
            } else {
                vec![]
            };

            LayerWeights {
                weight,
                bias,
                in_features,
                out_features,
            }
        })
        .collect()
}

/// Reconstruct Burn Linear layers from serializable weights.
pub fn build_linear_layers<Bk: burn::tensor::backend::Backend>(
    weights: &[LayerWeights],
    device: &Bk::Device,
) -> Vec<Linear<Bk>> {
    weights
        .iter()
        .map(|lw| {
            let config = burn::nn::LinearConfig::new(lw.in_features, lw.out_features);
            let mut linear = config.init(device);

            // Replace weight — Burn shape is [d_input, d_output].
            let w_f32: Vec<f32> = lw.weight.iter().map(|&v| v as f32).collect();
            let w_data = TensorData::new(w_f32, [lw.in_features, lw.out_features]);
            let w_tensor = Tensor::<Bk, 2>::from_data(w_data, device);
            linear.weight = Param::from_tensor(w_tensor);

            // Replace bias.
            if !lw.bias.is_empty() {
                let b_f32: Vec<f32> = lw.bias.iter().map(|&v| v as f32).collect();
                let b_data = TensorData::new(b_f32, [lw.out_features]);
                let b_tensor = Tensor::<Bk, 1>::from_data(b_data, device);
                linear.bias = Some(Param::from_tensor(b_tensor));
            }

            linear
        })
        .collect()
}
