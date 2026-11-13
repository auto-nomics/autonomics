//! One preparation path for BioArt raster and normalized, locally editable SVG.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Png,
    Svg,
}

#[derive(Debug, Serialize)]
pub struct PreparedAsset {
    pub asset: ::bioart::Asset,
    pub file: ::bioart::AssetFile,
    pub format: Format,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_sha256: Option<String>,
    pub width: u32,
    pub height: u32,
    #[serde(rename = "dataURL")]
    pub data_url: String,
    #[serde(rename = "previewDataURL", skip_serializing_if = "Option::is_none")]
    pub preview_data_url: Option<String>,
    pub colors: Vec<String>,
    pub normalization: Vec<String>,
    pub text_outlined: bool,
}
impl From<::bioart::CanvasImage> for PreparedAsset {
    fn from(image: ::bioart::CanvasImage) -> Self {
        Self {
            asset: image.asset,
            file: image.file,
            format: Format::Png,
            sha256: image.sha256,
            source_sha256: None,
            width: image.width,
            height: image.height,
            data_url: image.data_url,
            preview_data_url: None,
            colors: vec![],
            normalization: vec![],
            text_outlined: false,
        }
    }
}
impl PreparedAsset {
    pub fn preview_url(&self) -> &str {
        self.preview_data_url.as_deref().unwrap_or(&self.data_url)
    }
    pub fn metadata(&self) -> Value {
        json!({"asset":self.asset,"file":self.file,"format":self.format,"sha256":self.sha256,"source_sha256":self.source_sha256,"width":self.width,"height":self.height,"colors":self.colors,"normalization":self.normalization,"text_outlined":self.text_outlined})
    }
    pub fn provenance(&self) -> Value {
        let mut value =
            json!({"source":"NIH BioArt","asset":self.asset,"file":self.file,"sha256":self.sha256});
        if self.format == Format::Svg {
            value["source_sha256"] = json!(self.source_sha256);
            value["colors"] = json!(self.colors);
            value["normalization"] = json!(self.normalization);
        }
        value
    }
}
pub fn prepare_svg(
    asset: ::bioart::Asset,
    file: ::bioart::AssetFile,
    bytes: &[u8],
) -> Result<PreparedAsset> {
    if file.asset_id != asset.id || !file.format.eq_ignore_ascii_case("SVG") {
        return Err(Error::Invalid(
            "SVG file and BioArt asset identity must match",
        ));
    }
    let vector = ::bioicons::vector::normalize(bytes)?;
    Ok(PreparedAsset {
        asset,
        file,
        format: Format::Svg,
        sha256: vector.sha256,
        source_sha256: Some(vector.source_sha256),
        width: vector.width,
        height: vector.height,
        data_url: vector.data_url,
        preview_data_url: Some(vector.preview_data_url),
        colors: vector.colors,
        normalization: vector.normalization,
        text_outlined: vector.text_outlined,
    })
}
pub async fn prepare(
    client: &::bioart::BioArtClient,
    id: u64,
    format: Format,
) -> Result<PreparedAsset> {
    if format == Format::Png {
        return Ok(client.canvas_image(id).await?.into());
    }
    let detail = client.detail(id).await?;
    let file = detail
        .asset
        .files
        .iter()
        .chain(detail.representations.iter().flat_map(|r| r.files.iter()))
        .find(|file| file.format.eq_ignore_ascii_case("SVG"))
        .ok_or(Error::Invalid(
            "asset has no SVG representation; select PNG explicitly if desired",
        ))?;
    let download = client.download(file, 4 * 1024 * 1024).await?;
    prepare_svg(detail.asset, download.file, &download.bytes)
}
