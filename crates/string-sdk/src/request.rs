use serde::Serialize;

use crate::error::{Result, StringError};

/// Output representation requested from a text API endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Json,
    Tsv,
    TsvNoHeader,
    Xml,
    PsiMi,
    PsiMiTab,
}

impl OutputFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Tsv => "tsv",
            Self::TsvNoHeader => "tsv-no-header",
            Self::Xml => "xml",
            Self::PsiMi => "psi-mi",
            Self::PsiMiTab => "psi-mi-tab",
        }
    }
}

/// Image representation returned by visualization endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    HighResolutionPng,
    Svg,
}

impl ImageFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Png => "image",
            Self::HighResolutionPng => "highres_image",
            Self::Svg => "svg",
        }
    }

    /// Media type for the decoded image payload.
    pub fn media_type(self) -> &'static str {
        match self {
            Self::Svg => "image/svg+xml",
            Self::Png | Self::HighResolutionPng => "image/png",
        }
    }
}

/// Functional or physical network interpretation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkType {
    #[default]
    Functional,
    Physical,
}

impl NetworkType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Functional => "functional",
            Self::Physical => "physical",
        }
    }
}

/// Edge styling supported by network visualizations and links.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkFlavor {
    #[default]
    Evidence,
    Confidence,
    Actions,
}

impl NetworkFlavor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Evidence => "evidence",
            Self::Confidence => "confidence",
            Self::Actions => "actions",
        }
    }
}

/// Visualization-only options layered on top of a [`NetworkQuery`].
#[derive(Debug, Clone, Default)]
pub struct NetworkImageQuery {
    pub network: NetworkQuery,
    pub add_color_nodes: Option<u16>,
    pub add_white_nodes: Option<u16>,
    pub hide_node_labels: bool,
    pub hide_disconnected_nodes: bool,
    pub show_query_node_labels: bool,
    pub block_structure_pics_in_bubbles: bool,
    pub flat_node_design: bool,
    pub center_node_labels: bool,
    pub custom_label_font_size: Option<u8>,
}

impl NetworkImageQuery {
    pub fn new(network: NetworkQuery) -> Self {
        Self {
            network,
            ..Self::default()
        }
    }

    pub fn add_color_nodes(mut self, count: u16) -> Self {
        self.add_color_nodes = Some(count);
        self
    }

    pub fn add_white_nodes(mut self, count: u16) -> Self {
        self.add_white_nodes = Some(count);
        self
    }

    pub fn hide_node_labels(mut self, enabled: bool) -> Self {
        self.hide_node_labels = enabled;
        self
    }

    pub fn hide_disconnected_nodes(mut self, enabled: bool) -> Self {
        self.hide_disconnected_nodes = enabled;
        self
    }

    pub fn show_query_node_labels(mut self, enabled: bool) -> Self {
        self.show_query_node_labels = enabled;
        self
    }

    pub fn block_structure_pics_in_bubbles(mut self, enabled: bool) -> Self {
        self.block_structure_pics_in_bubbles = enabled;
        self
    }

    pub fn flat_node_design(mut self, enabled: bool) -> Self {
        self.flat_node_design = enabled;
        self
    }

    pub fn center_node_labels(mut self, enabled: bool) -> Self {
        self.center_node_labels = enabled;
        self
    }

    pub fn custom_label_font_size(mut self, size: u8) -> Self {
        self.custom_label_font_size = Some(size);
        self
    }
}

/// Parameters shared by network-oriented endpoints.
#[derive(Debug, Clone, Default)]
pub struct NetworkQuery {
    pub identifiers: Vec<String>,
    pub network_term_id: Option<String>,
    pub species: Option<String>,
    pub required_score: Option<u16>,
    pub network_type: NetworkType,
    pub add_nodes: Option<u16>,
    pub show_query_node_labels: bool,
    pub network_flavor: NetworkFlavor,
}

impl NetworkQuery {
    pub fn new<I, S>(identifiers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            identifiers: identifiers.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    /// Build a network from a functional term instead of explicit proteins.
    pub fn from_term(term: impl Into<String>) -> Self {
        Self {
            network_term_id: Some(term.into()),
            ..Self::default()
        }
    }

    pub fn species(mut self, species: impl Into<String>) -> Self {
        self.species = Some(species.into());
        self
    }

    pub fn required_score(mut self, score: u16) -> Self {
        self.required_score = Some(score);
        self
    }

    pub fn network_type(mut self, network_type: NetworkType) -> Self {
        self.network_type = network_type;
        self
    }

    pub fn add_nodes(mut self, count: u16) -> Self {
        self.add_nodes = Some(count);
        self
    }

    pub fn show_query_node_labels(mut self, enabled: bool) -> Self {
        self.show_query_node_labels = enabled;
        self
    }

    pub fn network_flavor(mut self, flavor: NetworkFlavor) -> Self {
        self.network_flavor = flavor;
        self
    }
}

/// Parameters for identifier resolution.
#[derive(Debug, Clone, Default)]
pub struct StringIdQuery {
    pub identifiers: Vec<String>,
    pub species: Option<String>,
    pub echo_query: bool,
}

impl StringIdQuery {
    pub fn new<I, S>(identifiers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            identifiers: identifiers.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    pub fn species(mut self, species: impl Into<String>) -> Self {
        self.species = Some(species.into());
        self
    }

    pub fn echo_query(mut self, enabled: bool) -> Self {
        self.echo_query = enabled;
        self
    }
}

/// Parameters for interaction partners.
#[derive(Debug, Clone, Default)]
pub struct InteractionPartnerQuery {
    pub identifiers: Vec<String>,
    pub species: Option<String>,
    pub limit: Option<u32>,
    pub required_score: Option<u16>,
    pub network_type: NetworkType,
}

impl InteractionPartnerQuery {
    pub fn new<I, S>(identifiers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            identifiers: identifiers.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    pub fn species(mut self, species: impl Into<String>) -> Self {
        self.species = Some(species.into());
        self
    }

    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn required_score(mut self, score: u16) -> Self {
        self.required_score = Some(score);
        self
    }

    pub fn network_type(mut self, network_type: NetworkType) -> Self {
        self.network_type = network_type;
        self
    }
}

/// Parameters for enrichment and PPI enrichment.
#[derive(Debug, Clone, Default)]
pub struct EnrichmentQuery {
    pub identifiers: Vec<String>,
    pub species: Option<String>,
    pub background_string_identifiers: Vec<String>,
}

impl EnrichmentQuery {
    pub fn new<I, S>(identifiers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            identifiers: identifiers.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    pub fn species(mut self, species: impl Into<String>) -> Self {
        self.species = Some(species.into());
        self
    }

    pub fn background<I, S>(mut self, identifiers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.background_string_identifiers = identifiers.into_iter().map(Into::into).collect();
        self
    }
}

/// Parameters for protein functional annotation.
#[derive(Debug, Clone, Default)]
pub struct AnnotationQuery {
    pub identifiers: Vec<String>,
    pub species: Option<String>,
    pub allow_pubmed: bool,
    pub only_pubmed: bool,
}

impl AnnotationQuery {
    pub fn new<I, S>(identifiers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            identifiers: identifiers.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    pub fn species(mut self, species: impl Into<String>) -> Self {
        self.species = Some(species.into());
        self
    }

    pub fn allow_pubmed(mut self, enabled: bool) -> Self {
        self.allow_pubmed = enabled;
        self
    }

    pub fn only_pubmed(mut self, enabled: bool) -> Self {
        self.only_pubmed = enabled;
        self
    }
}

/// Parameters for cross-species protein similarity.
#[derive(Debug, Clone, Default)]
pub struct HomologyQuery {
    pub identifiers: Vec<String>,
    pub species_b: Option<String>,
}

impl HomologyQuery {
    pub fn new<I, S>(identifiers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            identifiers: identifiers.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    pub fn species_b(mut self, species: impl Into<String>) -> Self {
        self.species_b = Some(species.into());
        self
    }
}

/// Validate and normalize non-empty identifier lists.
pub(crate) fn normalize_identifiers(
    identifiers: &[String],
    field: &'static str,
) -> Result<Vec<String>> {
    let mut normalized = Vec::with_capacity(identifiers.len());
    for identifier in identifiers {
        let identifier = identifier.trim();
        if identifier.is_empty() {
            return Err(StringError::InvalidRequest(format!(
                "{field} contains an empty identifier"
            )));
        }
        normalized.push(identifier.to_owned());
    }
    if normalized.is_empty() {
        return Err(StringError::InvalidRequest(format!(
            "{field} requires at least one identifier"
        )));
    }
    Ok(normalized)
}

/// Validate STRING's 0-1000 interaction significance score.
pub(crate) fn validate_required_score(score: u16) -> Result<()> {
    if score > 1000 {
        return Err(StringError::InvalidRequest(
            "required_score must be between 0 and 1000".into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_species(species: &Option<String>) -> Result<()> {
    if species
        .as_ref()
        .is_some_and(|species| species.trim().is_empty())
    {
        return Err(StringError::InvalidRequest(
            "species cannot be empty when supplied".into(),
        ));
    }
    Ok(())
}

pub(crate) fn joined<I>(values: &[I], separator: char) -> String
where
    I: ToString,
{
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(&separator.to_string())
}
