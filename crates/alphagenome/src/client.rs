use tonic::service::Interceptor;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, ClientTlsConfig};

use crate::error::{Error, Result};
use crate::pb::atlas as atlas_pb;
use crate::pb::v1 as pb;
use crate::query::{IntervalQuery, VariantQuery};
use crate::tensor;
use crate::types::{
    GeneInfo, ScoredOutput, ScorerMetadataEntry, TrackInfo, VariantScoreArray, VariantScores,
};

/// Default gRPC endpoint of the AlphaGenome API.
pub const DEFAULT_ENDPOINT: &str = "https://gdmscience.googleapis.com:443";

/// Environment variable holding the API key.
pub const ENV_API_KEY: &str = "ALPHAGENOME_API_KEY";

type AtlasChannel = InterceptedService<Channel, ApiKeyInterceptor>;

/// Async client for the AlphaGenome API (Atlas + DNA model services).
///
/// Create with [`AlphaGenomeClient::new`] or [`AlphaGenomeClient::from_env`].
#[derive(Debug, Clone)]
pub struct AlphaGenomeClient {
    atlas: atlas_pb::atlas_service_client::AtlasServiceClient<AtlasChannel>,
    model: pb::dna_model_service_client::DnaModelServiceClient<AtlasChannel>,
}

#[derive(Clone)]
struct ApiKeyInterceptor {
    api_key: String,
}

impl Interceptor for ApiKeyInterceptor {
    fn call(
        &mut self,
        mut request: tonic::Request<()>,
    ) -> std::result::Result<tonic::Request<()>, tonic::Status> {
        request.metadata_mut().insert(
            "x-goog-api-key",
            self.api_key.parse().map_err(|_| {
                tonic::Status::internal("API key contains invalid header characters")
            })?,
        );
        Ok(request)
    }
}

impl AlphaGenomeClient {
    /// Connects to the public endpoint with the given API key.
    pub async fn new(api_key: impl Into<String>) -> Result<Self> {
        Self::with_endpoint(DEFAULT_ENDPOINT, api_key).await
    }

    /// Connects using the `ALPHAGENOME_API_KEY` environment variable.
    pub async fn from_env() -> Result<Self> {
        let key =
            std::env::var(ENV_API_KEY).map_err(|_| Error::MissingApiKey(ENV_API_KEY.to_owned()))?;
        Self::new(key).await
    }

    /// Connects to an explicit endpoint (e.g. a local mock for tests).
    pub async fn with_endpoint(endpoint: &str, api_key: impl Into<String>) -> Result<Self> {
        let uri: tonic::transport::Uri = endpoint
            .parse()
            .map_err(|e| Error::Decode(format!("invalid endpoint {endpoint:?}: {e}")))?;
        let tls = ClientTlsConfig::new().with_enabled_roots();
        let channel = Channel::builder(uri)
            .tls_config(tls)
            .map_err(|e| Error::Grpc(tonic::Status::internal(format!("TLS config: {e}"))))?
            .connect()
            .await
            .map_err(|e| Error::Grpc(tonic::Status::unavailable(format!("connect failed: {e}"))))?;
        let interceptor = ApiKeyInterceptor {
            api_key: api_key.into(),
        };
        Ok(Self {
            atlas: atlas_pb::atlas_service_client::AtlasServiceClient::with_interceptor(
                channel.clone(),
                interceptor.clone(),
            ),
            model: pb::dna_model_service_client::DnaModelServiceClient::with_interceptor(
                channel,
                interceptor,
            ),
        })
    }

    // ------------------------------------------------------------------
    // Atlas: precomputed scores for every GRCh38 SNV
    // ------------------------------------------------------------------

    /// Lists the scorers the Atlas currently serves for an organism, with the
    /// number of values each returns per variant.
    pub async fn atlas_scorer_metadata(
        &mut self,
        organism: pb::Organism,
    ) -> Result<Vec<ScorerMetadataEntry>> {
        let request = atlas_pb::ListVariantScoresMetadataRequest {
            organism: organism as i32,
        };
        let response = self
            .atlas
            .list_variant_scores_metadata(tonic::Request::new(request))
            .await
            .map_err(Error::from_status)?;
        let response = response.into_inner();
        Ok(response
            .variant_scorer_metadata
            .into_iter()
            .map(|m| {
                let mut tracks = Vec::new();
                let mut num_values: Option<usize> = None;
                for metadata in &m.metadata {
                    match &metadata.payload {
                        Some(atlas_pb::metadata::Payload::Tracks(t)) => {
                            for track in &t.metadata {
                                tracks.push(crate::types::track_info(track));
                            }
                        }
                        Some(atlas_pb::metadata::Payload::GeneScorers(g)) => {
                            num_values = Some(g.metadata.len());
                        }
                        None => {}
                    }
                }
                if tracks.len() > 1 {
                    num_values = Some(tracks.len());
                }
                ScorerMetadataEntry {
                    name: m
                        .variant_scorer
                        .as_ref()
                        .map(|s| s.name.clone())
                        .unwrap_or_default(),
                    is_signed: m
                        .variant_scorer
                        .as_ref()
                        .map(|s| s.is_signed)
                        .unwrap_or(false),
                    num_values: num_values.unwrap_or(1),
                    tracks,
                }
            })
            .collect())
    }

    /// Fetches precomputed scores for a single GRCh38 SNV.
    ///
    /// `filter` is an AIP-160 filter over `DenseVariantScores`, most commonly
    /// `scores.variant_scorer.name = "AVI_SCORE"`; see
    /// [`Self::scorer_filter`] for building composite filters.
    pub async fn atlas_variant_scores(
        &mut self,
        variant: &VariantQuery,
        filter: Option<&str>,
    ) -> Result<VariantScores> {
        let request = atlas_pb::GetDenseVariantScoresRequest {
            variant: Some(variant.to_proto()),
            organism: pb::Organism::HomoSapiens as i32,
            filter: filter.unwrap_or_default().to_owned(),
        };
        let response = self
            .atlas
            .get_dense_variant_scores(tonic::Request::new(request))
            .await
            .map_err(Error::from_status)?
            .into_inner();
        Ok(dense_to_scores(response))
    }

    /// Streams precomputed scores for every scored SNV in a 1-based closed
    /// interval, following pagination automatically. Keep windows to ~1 kb;
    /// each base yields up to 3 variants.
    pub async fn atlas_interval_scores(
        &mut self,
        interval: &IntervalQuery,
        filter: Option<&str>,
        page_size: i32,
    ) -> Result<Vec<VariantScores>> {
        let mut all = Vec::new();
        let mut page_token = String::new();
        loop {
            let request = atlas_pb::ListDenseVariantScoresRequest {
                interval: Some(interval.to_proto()),
                organism: pb::Organism::HomoSapiens as i32,
                page_size,
                page_token: page_token.clone(),
                filter: filter.unwrap_or_default().to_owned(),
            };
            let response = self
                .atlas
                .list_dense_variant_scores(tonic::Request::new(request))
                .await
                .map_err(Error::from_status)?
                .into_inner();
            all.extend(response.variant_scores.into_iter().map(dense_to_scores));
            if response.next_page_token.is_empty() {
                break;
            }
            page_token = response.next_page_token;
        }
        Ok(all)
    }

    /// Builds an AIP-160 scorer filter, e.g.
    /// `(scores.variant_scorer.name = "AVI_SCORE" OR scores.variant_scorer.name = "RNA_SEQ")`.
    pub fn scorer_filter(scorers: &[&str]) -> Option<String> {
        if scorers.is_empty() {
            return None;
        }
        let clauses: Vec<String> = scorers
            .iter()
            .map(|s| format!("scores.variant_scorer.name = \"{s}\""))
            .collect();
        Some(if clauses.len() == 1 {
            clauses.into_iter().next().unwrap()
        } else {
            format!("({})", clauses.join(" OR "))
        })
    }

    // ------------------------------------------------------------------
    // DNA model: on-demand scoring
    // ------------------------------------------------------------------

    /// Scores a variant with the AlphaGenome model (human or mouse), for
    /// example indels or non-reference backgrounds the Atlas does not hold.
    ///
    /// `interval` must be one of the supported context windows (2^14 to 2^20
    /// bp); use [`Self::resize_1mb`] to expand around the variant. At most 20
    /// scorers per request.
    pub async fn score_variant(
        &mut self,
        interval: &IntervalQuery,
        variant: &VariantQuery,
        organism: pb::Organism,
        scorers: &[pb::variant_scorer::Scorer],
    ) -> Result<Vec<ScoredOutput>> {
        use pb::score_variant_response::Payload;

        let request = pb::ScoreVariantRequest {
            interval: Some(interval.to_proto()),
            variant: Some(variant.to_proto()),
            organism: organism as i32,
            variant_scorers: scorers
                .iter()
                .map(|s| pb::VariantScorer { scorer: Some(*s) })
                .collect(),
            model_version: String::new(),
            merge_stranded_gene_tracks: true,
        };
        let mut stream = self
            .model
            // ScoreVariant is a client-streaming RPC; a single request message
            // is the normal usage.
            .score_variant(tokio_stream::iter(vec![request]))
            .await
            .map_err(Error::from_status)?
            .into_inner();

        let mut outputs = Vec::new();
        let mut pending: Option<(
            pb::Tensor,
            Vec<pb::TensorChunk>,
            Vec<usize>,
            Vec<TrackInfo>,
            Vec<GeneInfo>,
            pb::Variant,
        )> = None;
        while let Some(message) = stream.message().await.map_err(Error::from_status)? {
            match message.payload {
                Some(Payload::Output(output)) => {
                    if let Some(entry) = pending.take() {
                        outputs.push(finish_output(entry)?);
                    }
                    let variant_data = output.variant_data.ok_or_else(|| {
                        Error::Decode("score output without variant data".to_owned())
                    })?;
                    let metadata = variant_data.metadata.clone().unwrap_or_default();
                    let tensor = variant_data.values.ok_or_else(|| {
                        Error::Decode("score output without a values tensor".to_owned())
                    })?;
                    pending = Some((
                        tensor,
                        Vec::new(),
                        shape_of(&metadata),
                        metadata
                            .track_metadata
                            .iter()
                            .map(crate::types::track_info)
                            .collect(),
                        metadata
                            .gene_metadata
                            .iter()
                            .map(crate::types::gene_info)
                            .collect(),
                        metadata
                            .variant
                            .clone()
                            .unwrap_or_else(|| variant.to_proto()),
                    ));
                }
                Some(Payload::TensorChunk(chunk)) => {
                    let entry = pending.as_mut().ok_or_else(|| {
                        Error::Decode("tensor chunk without a pending output".to_owned())
                    })?;
                    entry.1.push(chunk);
                }
                None => {}
            }
        }
        if let Some(entry) = pending.take() {
            outputs.push(finish_output(entry)?);
        }
        Ok(outputs)
    }

    /// Expands an interval around `variant` to the 1 Mb recommended context
    /// window (2^20), the default for full-context scoring.
    pub fn resize_1mb(variant: &VariantQuery) -> IntervalQuery {
        let half = (1i64 << 20) / 2;
        IntervalQuery::new(
            variant.chromosome.clone(),
            (variant.position - half).max(1),
            variant.position + half - 1,
        )
    }
}

fn shape_of(metadata: &pb::VariantMetadata) -> Vec<usize> {
    // Values are (num_tracks|num_genes, 1)-ish; the true shape comes from the
    // tensor header itself, which finish_output reads. This helper is only
    // used for metadata bookkeeping.
    let n = metadata
        .track_metadata
        .len()
        .max(metadata.gene_metadata.len());
    if n > 0 { vec![n] } else { vec![1] }
}

#[allow(clippy::type_complexity)]
fn finish_output(
    entry: (
        pb::Tensor,
        Vec<pb::TensorChunk>,
        Vec<usize>,
        Vec<TrackInfo>,
        Vec<GeneInfo>,
        pb::Variant,
    ),
) -> Result<ScoredOutput> {
    let (t, chunks, _meta_shape, tracks, genes, variant) = entry;
    let (shape, values) = tensor::decode_tensor(&t, &chunks)?;
    Ok(ScoredOutput {
        variant: variant.into(),
        shape,
        values,
        tracks,
        genes,
    })
}

fn dense_to_scores(dense: atlas_pb::DenseVariantScores) -> VariantScores {
    let scores = dense
        .scores
        .into_iter()
        .map(|s| {
            let shape: Vec<usize> = s.shape.iter().map(|&d| d as usize).collect();
            let mut tracks = Vec::new();
            let mut genes = Vec::new();
            for metadata in &s.metadata {
                match &metadata.payload {
                    Some(atlas_pb::metadata::Payload::Tracks(t)) => {
                        for track in &t.metadata {
                            tracks.push(crate::types::track_info(track));
                        }
                    }
                    Some(atlas_pb::metadata::Payload::GeneScorers(g)) => {
                        for gene in &g.metadata {
                            genes.push(crate::types::gene_info(gene));
                        }
                    }
                    None => {}
                }
            }
            let scorer_info = s.variant_scorer;
            VariantScoreArray {
                scorer: scorer_info
                    .as_ref()
                    .map(|v| v.name.clone())
                    .unwrap_or_default(),
                is_signed: scorer_info.map(|v| v.is_signed).unwrap_or(false),
                shape: shape.clone(),
                values: decode_f32(&s.scores, shape.iter().product()),
                calibrated_values: s
                    .calibrated_scores
                    .map(|b| decode_f32(&b, shape.iter().product())),
                tracks,
                genes,
            }
        })
        .collect();
    VariantScores {
        variant: dense
            .variant
            .map(Into::into)
            .unwrap_or_else(VariantScores::default_variant),
        interval: dense.interval.map(Into::into),
        scores,
    }
}

fn decode_f32(bytes: &[u8], expected_len: usize) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .take(expected_len.max(1))
        .collect()
}

impl VariantScores {
    fn default_variant() -> crate::types::VariantEcho {
        crate::types::VariantEcho {
            chromosome: String::new(),
            position: 0,
            reference_bases: String::new(),
            alternate_bases: String::new(),
        }
    }
}

/// Look up a scorer's per-track index by ontology CURIE (e.g. `UBERON:0001157`).
pub fn tracks_by_curie<'a>(
    scores: &'a VariantScoreArray,
    curie: &str,
) -> Vec<(usize, &'a TrackInfo)> {
    scores
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.ontology_curie.as_deref() == Some(curie))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scorer_filter_building() {
        assert_eq!(AlphaGenomeClient::scorer_filter(&[]), None);
        assert_eq!(
            AlphaGenomeClient::scorer_filter(&["AVI_SCORE"]),
            Some("scores.variant_scorer.name = \"AVI_SCORE\"".to_owned())
        );
        assert_eq!(
            AlphaGenomeClient::scorer_filter(&["AVI_SCORE", "RNA_SEQ"]),
            Some("(scores.variant_scorer.name = \"AVI_SCORE\" OR scores.variant_scorer.name = \"RNA_SEQ\")".to_owned())
        );
    }

    #[test]
    fn decode_f32_truncates_to_shape() {
        let bytes = 1.0f32
            .to_le_bytes()
            .iter()
            .copied()
            .chain(2.0f32.to_le_bytes().iter().copied())
            .collect::<Vec<u8>>();
        assert_eq!(decode_f32(&bytes, 1), vec![1.0]);
    }

    #[test]
    fn resize_1mb_centers_on_variant() {
        let v = VariantQuery::new("chr22", 36_201_698, "A", "C");
        let i = AlphaGenomeClient::resize_1mb(&v);
        assert_eq!(i.end - i.start + 1, 1 << 20);
        // 1-based closed interval whose 0-based half-open form is centered.
        assert_eq!(i.to_proto().start, v.position - 1 - (1 << 19));
    }
}
