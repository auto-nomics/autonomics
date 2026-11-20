//! Decoding of the API's chunked `Tensor` payloads into flat `Vec<f32>`.

use crate::error::{Error, Result};
use crate::pb::v1 as pb;

/// Decodes a [`pb::Tensor`] plus any trailing chunk messages into its shape
/// and a flat row-major `f32` buffer.
///
/// The wire protocol (see the Python SDK's `tensor_utils.pack_tensor`) splits
/// the flat buffer into byte ranges, compresses each chunk independently
/// (zstd or none), and streams the chunks after the message that carries the
/// `Tensor` header with `chunk_count` set.
pub fn decode_tensor(
    tensor: &pb::Tensor,
    chunks: &[pb::TensorChunk],
) -> Result<(Vec<usize>, Vec<f32>)> {
    let shape: Vec<usize> = tensor.shape.iter().map(|&d| d as usize).collect();
    let flat_len: usize = shape.iter().product();

    let bytes = match &tensor.payload {
        Some(pb::tensor::Payload::Array(array)) => {
            if !chunks.is_empty() {
                return Err(Error::Decode(
                    "tensor carries an inline array but chunk messages were also received"
                        .to_owned(),
                ));
            }
            decompress(array)?
        }
        Some(pb::tensor::Payload::ChunkCount(expected)) => {
            if chunks.len() != *expected as usize {
                return Err(Error::Decode(format!(
                    "expected {expected} tensor chunks, received {}",
                    chunks.len()
                )));
            }
            let mut buffer = Vec::new();
            for chunk in chunks {
                buffer.extend_from_slice(&decompress(chunk)?);
            }
            buffer
        }
        None => {
            // Scalar / empty payload.
            Vec::new()
        }
    };

    let values = decode_values(bytes.as_slice(), tensor.data_type(), flat_len)?;
    Ok((shape, values))
}

fn decompress(chunk: &pb::TensorChunk) -> Result<Vec<u8>> {
    match chunk.compression_type() {
        pb::CompressionType::None => Ok(chunk.data.clone()),
        pb::CompressionType::Zstd => zstd::decode_all(chunk.data.as_slice())
            .map_err(|e| Error::Decode(format!("zstd decompression failed: {e}"))),
    }
}

fn decode_values(bytes: &[u8], dtype: pb::DataType, expected_len: usize) -> Result<Vec<f32>> {
    let values: Vec<f32> = match dtype {
        pb::DataType::Float32 => bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect(),
        pb::DataType::Bfloat16 => bytes
            .chunks_exact(2)
            .map(|b| f32::from_bits((u16::from_le_bytes([b[0], b[1]]) as u32) << 16))
            .collect(),
        other => {
            return Err(Error::Decode(format!(
                "unsupported tensor dtype for scoring outputs: {other:?}"
            )));
        }
    };
    if values.len() != expected_len {
        return Err(Error::Decode(format!(
            "tensor shape implies {expected_len} values, decoded {}",
            values.len()
        )));
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tensor(dt: pb::DataType, shape: &[i32], payload: pb::tensor::Payload) -> pb::Tensor {
        pb::Tensor {
            shape: shape.to_vec(),
            data_type: dt as i32,
            payload: Some(payload),
        }
    }

    #[test]
    fn decodes_inline_float32() {
        let t = tensor(
            pb::DataType::Float32,
            &[2, 2],
            pb::tensor::Payload::Array(pb::TensorChunk {
                data: 1.0f32
                    .to_le_bytes()
                    .iter()
                    .chain(2.5f32.to_le_bytes().iter())
                    .chain((-1.0f32).to_le_bytes().iter())
                    .chain(0.0f32.to_le_bytes().iter())
                    .copied()
                    .collect(),
                compression_type: pb::CompressionType::None as i32,
            }),
        );
        let (shape, values) = decode_tensor(&t, &[]).unwrap();
        assert_eq!(shape, vec![2, 2]);
        assert_eq!(values, vec![1.0, 2.5, -1.0, 0.0]);
    }

    #[test]
    fn decodes_bfloat16() {
        // bfloat16 is the top 16 bits of f32; 1.0 => 0x3F80.
        let t = tensor(
            pb::DataType::Bfloat16,
            &[1],
            pb::tensor::Payload::Array(pb::TensorChunk {
                data: vec![0x80, 0x3F],
                compression_type: pb::CompressionType::None as i32,
            }),
        );
        let (_, values) = decode_tensor(&t, &[]).unwrap();
        assert_eq!(values, vec![1.0]);
    }

    #[test]
    fn reassembles_zstd_chunks() {
        let flat: Vec<u8> = (0..64u32).flat_map(|v| v.to_le_bytes()).collect();
        let mid = flat.len() / 2;
        let (a, b) = flat.split_at(mid);
        let mut ca = Vec::new();
        zstd::stream::encode_all(std::io::Cursor::new(a), 0).unwrap();
        // compress via block API for simplicity
        ca = zstd::bulk::compress(a, 0).unwrap();
        let cb = zstd::bulk::compress(b, 0).unwrap();
        let t = tensor(
            pb::DataType::Float32,
            &[64],
            pb::tensor::Payload::ChunkCount(2),
        );
        let chunks = vec![
            pb::TensorChunk {
                data: ca,
                compression_type: pb::CompressionType::Zstd as i32,
            },
            pb::TensorChunk {
                data: cb,
                compression_type: pb::CompressionType::Zstd as i32,
            },
        ];
        let (shape, values) = decode_tensor(&t, &chunks).unwrap();
        assert_eq!(shape, vec![64]);
        assert_eq!(values, (0..64u32).map(f32::from_bits).collect::<Vec<_>>());
    }

    #[test]
    fn rejects_chunk_count_mismatch() {
        let t = tensor(
            pb::DataType::Float32,
            &[2],
            pb::tensor::Payload::ChunkCount(2),
        );
        assert!(decode_tensor(&t, &[]).is_err());
    }
}
