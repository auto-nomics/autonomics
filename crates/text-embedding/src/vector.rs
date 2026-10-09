//! Dimension-checked vector math for clustering.

use crate::error::{EmbeddingError, Result};

/// L2-normalize in place. Zero vectors are left as zeros — cosine and
/// distance code guard on the zero norm instead of dividing by it.
pub(crate) fn normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for value in vector {
            *value /= norm;
        }
    }
}

/// Cosine similarity for two equal-length vectors.
pub fn cosine(left: &[f32], right: &[f32]) -> Result<f32> {
    if left.len() != right.len() {
        return Err(EmbeddingError::DimensionMismatch {
            expected: left.len(),
            actual: right.len(),
        });
    }
    let dot = left.iter().zip(right).map(|(a, b)| a * b).sum::<f32>();
    let norm = left.iter().map(|v| v * v).sum::<f32>().sqrt()
        * right.iter().map(|v| v * v).sum::<f32>().sqrt();
    Ok(if norm > f32::EPSILON { dot / norm } else { 0.0 })
}

/// Average equal-length vectors. Clustering uses this for centroids
/// and for choosing a stable representative.
pub fn mean_vector(vectors: &[&[f32]]) -> Result<Vec<f32>> {
    let Some((first, rest)) = vectors.split_first() else {
        return Err(EmbeddingError::NoVectors);
    };
    let mut mean = first.to_vec();
    for vector in rest.iter().copied() {
        if vector.len() != mean.len() {
            return Err(EmbeddingError::DimensionMismatch {
                expected: mean.len(),
                actual: vector.len(),
            });
        }
        for (sum, value) in mean.iter_mut().zip(vector) {
            *sum += value;
        }
    }
    let count = vectors.len() as f32;
    for value in &mut mean {
        *value /= count;
    }
    Ok(mean)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_detects_dimension_mismatch() {
        assert!(matches!(
            cosine(&[1.0], &[1.0, 0.0]),
            Err(EmbeddingError::DimensionMismatch { .. })
        ));
    }

    #[test]
    fn cosine_of_zero_vectors_is_zero_not_nan() {
        assert_eq!(cosine(&[0.0, 0.0], &[0.0, 0.0]).unwrap(), 0.0);
    }

    #[test]
    fn cosine_of_identical_vectors_is_one() {
        let similarity = cosine(&[0.6, 0.8], &[0.6, 0.8]).unwrap();
        assert!((similarity - 1.0).abs() < 1e-6);
    }

    #[test]
    fn mean_vector_requires_equal_dimensions() {
        assert!(mean_vector(&[&[1.0], &[1.0, 0.0]]).is_err());
    }

    #[test]
    fn mean_vector_rejects_an_empty_batch() {
        assert!(matches!(mean_vector(&[]), Err(EmbeddingError::NoVectors)));
    }

    #[test]
    fn mean_vector_averages_members() {
        assert_eq!(
            mean_vector(&[&[1.0, 3.0], &[3.0, 1.0]]).unwrap(),
            [2.0, 2.0]
        );
    }
}
