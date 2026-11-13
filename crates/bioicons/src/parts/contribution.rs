//! Bounded leave-one-path-out render diagnostics, never an exact visibility mask.
use super::{Part, attributes, element, invalid, rewrite};
use crate::Result;
use resvg::{tiny_skia, usvg};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RenderContribution {
    Completed {
        width: u32,
        height: u32,
        pixel_channel_tolerance: u8,
        scope: &'static str,
    },
    NotComputed {
        reason: &'static str,
    },
}

pub(super) fn analyze(
    canonical: &[u8],
    tree: &usvg::Tree,
    parts: &mut [Part],
) -> Result<RenderContribution> {
    if parts.len() > 32 || canonical.len().saturating_mul(parts.len()) > 8 * 1024 * 1024 {
        return Ok(RenderContribution::NotComputed {
            reason: "limited to 32 paths and 8 MiB of cumulative SVG parse input; no path visibility inferred",
        });
    }
    let size = tree.size();
    let scale = (256. / size.width().max(size.height())).min(1.);
    let width = (size.width() * scale).ceil().max(1.) as u32;
    let height = (size.height() * scale).ceil().max(1.) as u32;
    let render = |tree: &usvg::Tree| -> Result<tiny_skia::Pixmap> {
        let mut pixels = tiny_skia::Pixmap::new(width, height)
            .ok_or_else(|| invalid("invalid contribution preview size"))?;
        resvg::render(
            tree,
            tiny_skia::Transform::from_scale(scale, scale),
            &mut pixels.as_mut(),
        );
        Ok(pixels)
    };
    let baseline = render(tree)?;
    for part in parts {
        let hidden = rewrite(canonical, |path, index| {
            let mut attrs = attributes(path)?;
            if index == part.index {
                attrs.insert("opacity".into(), "0".into());
            }
            Ok(element(attrs))
        })?;
        let hidden = usvg::Tree::from_data(&hidden, &usvg::Options::default())
            .map_err(|e| invalid(e.to_string()))?;
        let pixels = render(&hidden)?;
        let changed = baseline
            .data()
            .chunks_exact(4)
            .zip(pixels.data().chunks_exact(4))
            .filter(|(a, b)| a.iter().zip(*b).any(|(x, y)| x.abs_diff(*y) > 1))
            .count();
        part.changed_preview_pixels_when_hidden = Some(changed as u32);
    }
    Ok(RenderContribution::Completed {
        width,
        height,
        pixel_channel_tolerance: 1,
        scope: "Hide one path at a time and compare premultiplied RGBA on a transparent SVG root preview at this bounded resolution. Excludes canvas crop, scale, background and other scene objects. Counts measure render contribution, including filters/compositing, not visible area or a pixel mask. Zero can mean occlusion, identical paint, clipping, transparency, or subpixel detail; inspect the PNG. Does not modify the stored SVG.",
    })
}
