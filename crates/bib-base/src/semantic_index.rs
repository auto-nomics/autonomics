//! Durable, model-bound vectors derived from the canonical search projection.
//! The owning worker must serialize calls on the two dedicated connections.
//! No network operation holds a canonical read transaction or a vector write
//! transaction. A generation becomes visible only after every batch succeeds.
use crate::semantic_limits::{self, SemanticLimits, SemanticResourceUsage};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use text_embedding::{EmbeddingInput, TextEmbedder};
use turso::{
    Connection, Value,
    transaction::{Transaction, TransactionBehavior},
};

mod cleanup;
#[cfg(test)]
mod tests;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS semanticHeads (
 signature TEXT PRIMARY KEY,
 generation TEXT NOT NULL REFERENCES semanticGenerations(generation)
);
CREATE INDEX IF NOT EXISTS semanticHeads_generation ON semanticHeads(generation);
CREATE TABLE IF NOT EXISTS semanticGenerations (
 generation TEXT PRIMARY KEY, signature TEXT NOT NULL,
 revision INTEGER NOT NULL, token TEXT NOT NULL, dimensions INTEGER NOT NULL,
 touchedAt INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS semanticGenerations_signature ON semanticGenerations(signature);
CREATE TABLE IF NOT EXISTS semanticVectors (
 signature TEXT NOT NULL, inputHash TEXT NOT NULL,
 vector BLOB NOT NULL, checksum TEXT NOT NULL,
 PRIMARY KEY(signature,inputHash)
);
CREATE TABLE IF NOT EXISTS semanticBindings (
 generation TEXT NOT NULL REFERENCES semanticGenerations(generation) ON DELETE CASCADE,
 chunkID TEXT NOT NULL, docID TEXT NOT NULL, articleID TEXT NOT NULL,
 anchorKey TEXT NOT NULL, kind TEXT NOT NULL, contentHash TEXT NOT NULL,
 startByte INTEGER NOT NULL, endByte INTEGER NOT NULL, inputHash TEXT NOT NULL,
 PRIMARY KEY(generation,chunkID)
);
CREATE INDEX IF NOT EXISTS semanticBindings_article ON semanticBindings(generation,articleID);
CREATE INDEX IF NOT EXISTS semanticBindings_input ON semanticBindings(inputHash,generation);
CREATE TABLE IF NOT EXISTS semanticBindingAliases (
 generation TEXT PRIMARY KEY REFERENCES semanticGenerations(generation) ON DELETE CASCADE,
 bindingGeneration TEXT NOT NULL REFERENCES semanticGenerations(generation),
 CHECK(generation<>bindingGeneration)
);
CREATE INDEX IF NOT EXISTS semanticBindingAliases_source ON semanticBindingAliases(bindingGeneration);
CREATE TABLE IF NOT EXISTS semanticGarbageCollection(signature TEXT PRIMARY KEY,afterInputHash TEXT NOT NULL);
"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SemanticProfile {
    pub dimensions: usize,
    pub query_instruction: String,
    /// None embeds title + complete abstract. Windows count Unicode codepoints.
    pub chunk_chars: Option<usize>,
    pub overlap_chars: usize,
    pub include_annotations: bool,
    pub include_fulltext: bool,
}
impl Default for SemanticProfile {
    fn default() -> Self {
        Self { dimensions:1024,
            query_instruction:"Given a scientific claim, retrieve scientific papers that provide relevant evidence.".into(),
            chunk_chars:None, overlap_chars:0, include_annotations:false, include_fulltext:false }
    }
}
impl SemanticProfile {
    pub fn validate(&self) -> Result<()> {
        if !(1..=65536).contains(&self.dimensions)
            || self.query_instruction.len() > 4096
            || self
                .chunk_chars
                .is_some_and(|size| !(32..=16384).contains(&size) || self.overlap_chars >= size)
            || (self.chunk_chars.is_none()
                && (self.overlap_chars != 0 || self.include_annotations || self.include_fulltext))
        {
            return Err(invalid(
                "invalid semantic dimensions, instruction, or chunk policy; evidence indexing requires windows",
            ));
        }
        Ok(())
    }
    pub fn signature(&self, provider: &dyn TextEmbedder) -> Result<String> {
        self.signature_for(&provider.signature())
    }
    fn signature_for(&self, provider_signature: &str) -> Result<String> {
        self.validate()?;
        let bytes = serde_json::to_vec(&(
            "bib-semantic-v1:title-newline-body:utf8-byte-windows",
            provider_signature,
            self,
        ))?;
        Ok(hash(&bytes))
    }
    pub fn query_input(&self, text: &str) -> Result<EmbeddingInput> {
        if text.trim().is_empty() || text.len() > 16384 {
            return Err(invalid("semantic query requires 1 to 16384 bytes"));
        }
        let text = if self.query_instruction.is_empty() {
            text.to_owned()
        } else {
            format!("Instruct: {}\nQuery: {text}", self.query_instruction)
        };
        Ok(EmbeddingInput::new(hash(text.as_bytes()), text))
    }
}

#[derive(Debug, Clone)]
pub struct SemanticChunk {
    pub chunk_id: String,
    pub doc_id: String,
    pub article_id: String,
    pub anchor_key: String,
    pub kind: String,
    /// Hash of the complete title + newline + canonical body, before windowing.
    pub content_hash: String,
    /// UTF-8 offsets into the canonical projection's abstract/body, not PDF
    /// coordinates. Annotation body includes comment/tags in projection order.
    pub start_byte: usize,
    pub end_byte: usize,
    pub vector: Arc<Vec<f32>>,
}

#[derive(Debug, Clone)]
pub struct SemanticSnapshot {
    pub signature: String,
    pub generation: String,
    pub revision: u64,
    pub token: String,
    pub dimensions: usize,
    pub chunks: Vec<SemanticChunk>,
}

impl SemanticSnapshot {
    /// Hard constraints and library/collection permissions supply `allowed`
    /// before Top-K. Same-article chunks aggregate by maximum cosine.
    pub fn rank(
        &self,
        query: &[f32],
        allowed: &HashSet<String>,
        limit: usize,
    ) -> Result<Vec<(SemanticChunk, f64)>> {
        let query = normalized(query, self.dimensions)?;
        fn collect<'a>(
            chunks: &'a [SemanticChunk],
            query: &[f32],
            allowed: &HashSet<String>,
        ) -> HashMap<&'a str, (&'a SemanticChunk, f64)> {
            let mut matches = HashMap::new();
            for chunk in chunks {
                if !allowed.contains(&chunk.article_id) {
                    continue;
                }
                // Each chunk retains the original serial f64 accumulation.
                let score = chunk
                    .vector
                    .iter()
                    .zip(query)
                    .map(|(a, b)| f64::from(*a) * f64::from(*b))
                    .sum::<f64>();
                let entry = matches
                    .entry(chunk.article_id.as_str())
                    .or_insert((chunk, score));
                if score > entry.1 || (score == entry.1 && chunk.chunk_id < entry.0.chunk_id) {
                    *entry = (chunk, score);
                }
            }
            matches
        }
        let workers = if self.chunks.len() >= 65_536 && allowed.len() >= 16_384 {
            std::thread::available_parallelism().map_or(1, |n| n.get().min(4))
        } else {
            1
        };
        let matches = if workers > 1 {
            // The search actor already serializes ranking requests. Bound CPU
            // fanout rather than create a machine-wide pool competing with the
            // model/index workers. Narrow scopes avoid thread setup entirely.
            std::thread::scope(|scope| {
                let jobs = self
                    .chunks
                    .chunks(self.chunks.len().div_ceil(workers))
                    .map(|chunks| {
                        let query = &query;
                        scope.spawn(move || collect(chunks, query, allowed))
                    })
                    .collect::<Vec<_>>();
                let mut matches = HashMap::new();
                for job in jobs {
                    for (article, candidate) in job
                        .join()
                        .map_err(|_| Error::Unknown("semantic ranking worker panicked".into()))?
                    {
                        let entry = matches.entry(article).or_insert(candidate);
                        if candidate.1 > entry.1
                            || (candidate.1 == entry.1 && candidate.0.chunk_id < entry.0.chunk_id)
                        {
                            *entry = candidate;
                        }
                    }
                }
                Ok::<_, Error>(matches)
            })?
        } else {
            collect(&self.chunks, &query, allowed)
        };
        let mut result = matches.into_values().collect::<Vec<_>>();
        let order = |a: &(&SemanticChunk, f64), b: &(&SemanticChunk, f64)| {
            b.1.total_cmp(&a.1)
                .then_with(|| a.0.article_id.cmp(&b.0.article_id))
        };
        // Keep the exact article maximum and tie order, but avoid copying all
        // evidence strings or sorting the whole eligible library for Top-K.
        if result.len() > limit {
            result.select_nth_unstable_by(limit, order);
            result.truncate(limit);
        }
        result.sort_unstable_by(order);
        Ok(result
            .into_iter()
            .map(|(chunk, score)| (chunk.clone(), score))
            .collect())
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct SemanticSyncReport {
    pub rebuilt: bool,
    pub changed_articles: usize,
    pub embedded_chunks: usize,
    pub cached_chunks: usize,
    pub total_chunks: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SemanticProgress {
    /// planning, staging, copying_bindings, embedding, publishing, complete.
    /// Counts include only completed embedding
    /// batches; a model call in flight is never reported as committed work.
    pub stage: String,
    pub target_revision: Option<u64>,
    pub target_articles: Option<usize>,
    pub processed_articles: usize,
    pub processed_chunks: usize,
    pub embedded_chunks: usize,
    pub cached_chunks: usize,
}

struct Head {
    generation: String,
    revision: u64,
    token: String,
}
struct InputChunk {
    id: String,
    doc_id: String,
    article_id: String,
    anchor: String,
    kind: String,
    content_hash: String,
    start: usize,
    end: usize,
    input: EmbeddingInput,
}

pub struct SemanticIndex {
    source: Connection,
    store: Connection,
    provider: Arc<dyn TextEmbedder>,
    profile: SemanticProfile,
    signature: String,
    provider_signature: String,
    staging: Option<String>,
    cleanup_warning: Option<String>,
    current: Option<Arc<SemanticSnapshot>>,
    limits: SemanticLimits,
    disk_family: Option<std::path::PathBuf>,
}

async fn reap_expired_generations(store: &Connection) -> Result<()> {
    cleanup::reap(store).await
}
async fn delete_unused_generation(store: &Connection, generation: &str) -> Result<()> {
    cleanup::retire(store, generation).await
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn invalid(message: &str) -> Error {
    Error::InvalidSearchQuery(message.into())
}
fn conflict() -> Error {
    Error::SearchConflict("semantic source or index changed while building; retry".into())
}
fn normalized(vector: &[f32], dimensions: usize) -> Result<Vec<f32>> {
    if vector.len() != dimensions || vector.iter().any(|v| !v.is_finite()) {
        return Err(invalid("semantic vector has invalid dimensions or values"));
    }
    let norm = vector
        .iter()
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>()
        .sqrt();
    if norm <= 1e-12 || !norm.is_finite() {
        return Err(invalid("semantic vector has zero or invalid norm"));
    }
    Ok(vector
        .iter()
        .map(|v| (f64::from(*v) / norm) as f32)
        .collect())
}
pub(crate) fn validate_query_vector(vector: &[f32], dimensions: usize) -> Result<()> {
    normalized(vector, dimensions).map(|_| ())
}
fn encode(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn decode(bytes: &[u8], checksum: &str, dimensions: usize) -> Result<Vec<f32>> {
    if bytes.len() != dimensions * 4 || hash(bytes) != checksum {
        return Err(invalid("semantic vector cache checksum mismatch"));
    }
    let vector = bytes
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four bytes")))
        .collect::<Vec<_>>();
    let unit = normalized(&vector, dimensions)?;
    if vector
        .iter()
        .zip(&unit)
        .any(|(a, b)| (*a - *b).abs() > 1e-4)
    {
        return Err(invalid("semantic cache vector is not normalized"));
    }
    Ok(vector)
}

pub(crate) async fn source_version(conn: &Connection) -> Result<(u64, String)> {
    let mut rows=conn.query("SELECT key,value FROM bib_meta WHERE key IN ('search_revision','search_revision_token')",()).await?;
    let mut revision = None;
    let mut token = None;
    while let Some(row) = rows.next().await? {
        let key: String = row.get(0)?;
        let value: String = row.get(1)?;
        if key == "search_revision" {
            revision = Some(
                value
                    .parse::<u64>()
                    .map_err(|_| invalid("invalid source search revision"))?,
            );
        } else {
            token = Some(value);
        }
    }
    Ok((
        revision.ok_or_else(|| invalid("source search revision missing"))?,
        token.ok_or_else(|| invalid("source search token missing"))?,
    ))
}

impl SemanticIndex {
    /// Use a separate canonical reader and sidecar. Opening never contacts a
    /// model server. The caller explicitly supplies an installed provider.
    pub async fn open(
        source: Connection,
        path: &str,
        profile: SemanticProfile,
        provider: Arc<dyn TextEmbedder>,
    ) -> Result<Self> {
        let provider_signature = provider.signature();
        let signature = profile.signature_for(&provider_signature)?;
        let mut builder = turso::Builder::new_local(path);
        if path != ":memory:" {
            builder = builder.experimental_multiprocess_wal(true);
        }
        let db = builder.build().await?;
        let store = db.connect()?;
        store.pragma_update("foreign_keys", true).await?;
        if path != ":memory:" {
            store.pragma_update("busy_timeout", 5000).await?;
        }
        store.execute_batch(SCHEMA).await?;
        semantic_limits::initialize_store(&store).await?;
        // An expired build may be discarded; published heads are never reaped.
        // A suspended writer resuming after expiry fails its FK/CAS checks.
        let cleanup_warning = reap_expired_generations(&store)
            .await
            .err()
            .map(|error| format!("semantic cleanup pending: {error}"));
        Ok(Self {
            source,
            store,
            provider,
            profile,
            signature,
            provider_signature,
            staging: None,
            cleanup_warning,
            current: None,
            limits: SemanticLimits::default(),
            disk_family: (path != ":memory:").then(|| path.into()),
        })
    }

    pub fn with_limits(mut self, limits: SemanticLimits) -> Result<Self> {
        limits.validate()?;
        self.limits = limits;
        Ok(self)
    }

    pub(crate) fn with_disk_family(mut self, family: &str) -> Self {
        self.disk_family = (family != ":memory:").then(|| family.into());
        self
    }

    pub async fn resource_usage(&self) -> Result<SemanticResourceUsage> {
        Ok(SemanticResourceUsage {
            vector_bytes: semantic_limits::vector_bytes(&self.store).await?,
            disk_bytes: semantic_limits::disk_bytes(self.disk_family.as_deref()).await?,
        })
    }

    async fn check_resources(&self, additional_vector_bytes: u64) -> Result<()> {
        let usage = self.resource_usage().await?;
        let bytes = usage
            .vector_bytes
            .checked_add(additional_vector_bytes)
            .ok_or_else(|| invalid("semantic vector size overflow"))?;
        semantic_limits::check(
            "vector payload bytes per store",
            bytes,
            self.limits.max_vector_bytes,
        )?;
        if let Some(bytes) = usage.disk_bytes {
            semantic_limits::check(
                "semantic cache disk bytes",
                bytes,
                self.limits.max_disk_bytes,
            )?;
        }
        Ok(())
    }

    pub fn signature(&self) -> &str {
        &self.signature
    }

    async fn head(&self) -> Result<Option<Head>> {
        let mut rows=self.store.query("SELECT g.generation,g.revision,g.token,g.dimensions,g.signature FROM semanticHeads h LEFT JOIN semanticGenerations g USING(generation) WHERE h.signature=?1",turso::params![self.signature.clone()]).await?;
        rows.next()
            .await?
            .map(|row| -> Result<Head> {
                let revision = row.get::<i64>(1)?;
                if revision < 0
                    || row.get::<i64>(3)? != self.profile.dimensions as i64
                    || row.get::<String>(4)? != self.signature
                {
                    return Err(invalid("invalid semantic generation metadata"));
                }
                Ok(Head {
                    generation: row.get(0)?,
                    revision: revision as u64,
                    token: row.get(2)?,
                })
            })
            .transpose()
    }

    /// Aliases always point directly at a materialized binding set. Keeping
    /// that generation alive avoids copying unchanged rows on metadata edits.
    async fn binding_generation(&self, generation: &str) -> Result<String> {
        let mut rows=self.store.query("SELECT a.bindingGeneration,g.signature,g.dimensions FROM semanticBindingAliases a LEFT JOIN semanticGenerations g ON g.generation=a.bindingGeneration WHERE a.generation=?1",turso::params![generation.to_owned()]).await?;
        let Some(row) = rows.next().await? else {
            return Ok(generation.to_owned());
        };
        let binding: String = row.get(0)?;
        if row.get::<String>(1)? != self.signature
            || row.get::<i64>(2)? != self.profile.dimensions as i64
        {
            return Err(invalid("semantic binding alias profile mismatch"));
        }
        drop(rows);
        let mut rows = self
            .store
            .query(
                "SELECT 1 FROM semanticBindingAliases WHERE generation=?1",
                turso::params![binding.clone()],
            )
            .await?;
        if rows.next().await?.is_some() {
            return Err(invalid("nested semantic binding alias"));
        }
        Ok(binding)
    }

    async fn metadata_only(
        &mut self,
        old: &Head,
        revision: u64,
        token: &str,
        articles: &[String],
    ) -> Result<Option<(Arc<SemanticSnapshot>, SemanticSyncReport)>> {
        let Some(base) = self
            .current
            .clone()
            .filter(|base| base.generation == old.generation)
        else {
            return Ok(None);
        };
        let changed: HashSet<_> = articles.iter().collect();
        let mut remaining: HashSet<_> = base
            .chunks
            .iter()
            .filter(|chunk| changed.contains(&chunk.article_id))
            .map(|chunk| chunk.chunk_id.clone())
            .collect();
        let mut reused = 0;
        for batch in articles.chunks(32) {
            for input in self
                .read_inputs(batch, revision, token, self.limits.max_chunks)
                .await?
            {
                if !remaining.remove(&input.id) {
                    return Ok(None);
                }
                reused += 1;
            }
        }
        if !remaining.is_empty() {
            return Ok(None);
        };
        if source_version(&self.source).await? != (revision, token.to_owned()) {
            return Err(conflict());
        }
        if self.provider.signature() != self.provider_signature {
            return Err(invalid("semantic provider signature changed"));
        }
        let generation = uuid::Uuid::new_v4().to_string();
        let snapshot = Arc::new(SemanticSnapshot {
            signature: self.signature.clone(),
            generation: generation.clone(),
            revision,
            token: token.to_owned(),
            dimensions: self.profile.dimensions,
            chunks: base.chunks.clone(),
        });
        let tx = Transaction::new_unchecked(&self.store, TransactionBehavior::Immediate).await?;
        let publish = async {
            if self
                .head()
                .await?
                .as_ref()
                .map(|head| head.generation.as_str())
                != Some(old.generation.as_str())
            {
                return Err(conflict());
            }
            let binding = self.binding_generation(&old.generation).await?;
            let mut rows = self
                .store
                .query(
                    "SELECT COUNT(*) FROM semanticBindings WHERE generation=?1",
                    turso::params![binding.clone()],
                )
                .await?;
            if rows
                .next()
                .await?
                .ok_or_else(|| invalid("missing semantic binding count"))?
                .get::<i64>(0)?
                != base.chunks.len() as i64
            {
                return Err(invalid(
                    "semantic metadata alias references missing bindings",
                ));
            }
            drop(rows);
            let revision_db = i64::try_from(revision)
                .map_err(|_| invalid("semantic revision exceeds storage range"))?;
            self.store
                .execute(
                    "INSERT INTO semanticGenerations VALUES(?1,?2,?3,?4,?5,?6)",
                    turso::params![
                        generation.clone(),
                        self.signature.clone(),
                        revision_db,
                        token.to_owned(),
                        self.profile.dimensions as i64,
                        chrono::Utc::now().timestamp_millis()
                    ],
                )
                .await?;
            self.store
                .execute(
                    "INSERT INTO semanticBindingAliases VALUES(?1,?2)",
                    turso::params![generation.clone(), binding],
                )
                .await?;
            self.store
                .execute(
                    "UPDATE semanticHeads SET generation=?1 WHERE signature=?2",
                    turso::params![generation.clone(), self.signature.clone()],
                )
                .await?;
            delete_unused_generation(&self.store, &old.generation).await?;
            Ok(())
        }
        .await;
        match publish {
            Ok(()) => tx.commit().await?,
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(error);
            }
        }
        self.current = Some(snapshot.clone());
        Ok(Some((
            snapshot,
            SemanticSyncReport {
                rebuilt: false,
                changed_articles: articles.len(),
                embedded_chunks: 0,
                cached_chunks: reused,
                total_chunks: base.chunks.len(),
            },
        )))
    }

    /// One source snapshot supplies revision and changed identities. Text is
    /// read in bounded article batches and the version is rechecked each time.
    async fn plan(&self, old: Option<&Head>) -> Result<(u64, String, bool, Vec<String>)> {
        let tx = Transaction::new_unchecked(&self.source, TransactionBehavior::Deferred).await?;
        let result = async {
            let (revision, token) = source_version(&self.source).await?;
            let rebuild = old.is_none_or(|old| {
                match old.revision.cmp(&revision) {
                    std::cmp::Ordering::Greater => true,
                    std::cmp::Ordering::Equal => old.token != token,
                    // A higher revision on a fresh handle can come from a
                    // restored branch. Its outbox does not prove ancestry.
                    std::cmp::Ordering::Less => self.current.is_none(),
                }
            });
            let mut ids = Vec::new();
            if rebuild {
                let mut rows = self
                    .source
                    .query("SELECT id FROM articles ORDER BY id", ())
                    .await?;
                while let Some(row) = rows.next().await? {
                    ids.push(row.get(0)?);
                }
            } else if old.is_some_and(|old| old.revision != revision || old.token != token) {
                let mut rows = self
                    .source
                    .query(
                        "SELECT articleID FROM searchOutbox WHERE revision>?1 ORDER BY articleID",
                        turso::params![old.map(|old| old.revision as i64).unwrap_or(0)],
                    )
                    .await?;
                while let Some(row) = rows.next().await? {
                    ids.push(row.get(0)?);
                }
                if ids.is_empty() {
                    let mut rows = self
                        .source
                        .query("SELECT id FROM articles ORDER BY id", ())
                        .await?;
                    while let Some(row) = rows.next().await? {
                        ids.push(row.get(0)?);
                    }
                    return Ok((revision, token, true, ids));
                }
            }
            Ok((revision, token, rebuild, ids))
        }
        .await;
        match result {
            Ok(plan) => {
                tx.commit().await?;
                Ok(plan)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }

    async fn read_inputs(
        &self,
        articles: &[String],
        revision: u64,
        token: &str,
        max_chunks: usize,
    ) -> Result<Vec<InputChunk>> {
        let tx = Transaction::new_unchecked(&self.source, TransactionBehavior::Deferred).await?;
        let result=async {
            if source_version(&self.source).await?!=(revision,token.to_owned()) { return Err(conflict()); }
            let placeholders=(1..=articles.len()).map(|i|format!("?{i}")).collect::<Vec<_>>().join(",");
            let projection=crate::search_projection::select(Some(&format!("a.id IN ({placeholders})")));
            let sql=format!("SELECT docID,articleID,anchorKey,kind,title,abstract,body FROM ({projection}) WHERE kind='item'{}{} ORDER BY docID",if self.profile.include_annotations{" OR kind='annotation'"}else{""},if self.profile.include_fulltext{" OR kind='attachment'"}else{""});
            let mut rows=self.source.query(sql,turso::params_from_iter(articles.iter().cloned().map(Value::Text))).await?;
            let mut result=Vec::new();
            while let Some(row)=rows.next().await? {
                let doc_id:String=row.get(0)?; let article_id:String=row.get(1)?;
                let anchor:String=row.get(2)?; let kind:String=row.get(3)?;
                let title:String=row.get(4)?;
                let body:String=row.get(if kind=="item"{5}else{6})?;
                if body.trim().is_empty() && (kind!="item" || title.trim().is_empty()) { continue; }
                let content_hash=hash(format!("{title}\n{body}").as_bytes());
                let chars=body.chars().count();
                let count=match self.profile.chunk_chars {
                    Some(size) if chars>size => 1+(chars-size).div_ceil(size-self.profile.overlap_chars),
                    _=>1,
                };
                let retained=self.limits.max_chunks.saturating_sub(max_chunks);
                semantic_limits::check("index chunks",(retained+result.len()+count) as u64,self.limits.max_chunks as u64)?;
                for (start,end) in windows(&body,self.profile.chunk_chars,self.profile.overlap_chars) {
                    semantic_limits::check("model input bytes",(title.len()+1+end-start) as u64,self.limits.max_input_bytes as u64)?;
                    let text=format!("{title}\n{}",&body[start..end]);
                    let input_hash=hash(text.as_bytes());
                    let id=hash(serde_json::to_string(&(&doc_id,&content_hash,start,end,&input_hash))?.as_bytes());
                    result.push(InputChunk{id,doc_id:doc_id.clone(),article_id:article_id.clone(),anchor:anchor.clone(),kind:kind.clone(),content_hash:content_hash.clone(),start,end,input:EmbeddingInput::new(input_hash,text)});
                }
            }
            Ok(result)
        }.await;
        match result {
            Ok(inputs) => {
                tx.commit().await?;
                Ok(inputs)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }

    async fn cached(&self, input: &str) -> Result<Option<Vec<f32>>> {
        let mut rows = self
            .store
            .query(
                "SELECT vector,checksum FROM semanticVectors WHERE signature=?1 AND inputHash=?2",
                turso::params![self.signature.clone(), input.to_owned()],
            )
            .await?;
        rows.next()
            .await?
            .map(|row| {
                decode(
                    &row.get::<Vec<u8>>(0)?,
                    &row.get::<String>(1)?,
                    self.profile.dimensions,
                )
            })
            .transpose()
    }

    async fn append(
        &self,
        generation: &str,
        chunks: &[InputChunk],
        report: &mut SemanticSyncReport,
        shared_vectors: &mut HashMap<String, Arc<Vec<f32>>>,
    ) -> Result<Vec<SemanticChunk>> {
        if self.provider.signature() != self.provider_signature {
            return Err(invalid("semantic provider signature changed"));
        }
        let mut pending = Vec::new();
        let mut vectors = HashMap::new();
        for chunk in chunks {
            if vectors.contains_key(&chunk.input.id) {
                report.cached_chunks += 1;
                continue;
            }
            if let Some(vector) = self.cached(&chunk.input.id).await? {
                vectors.insert(chunk.input.id.clone(), vector);
                report.cached_chunks += 1;
            } else if !pending
                .iter()
                .any(|input: &EmbeddingInput| input.id == chunk.input.id)
            {
                pending.push(chunk.input.clone());
            } else {
                report.cached_chunks += 1;
            }
        }
        if !pending.is_empty() {
            self.check_resources((pending.len() * self.profile.dimensions * 4) as u64)
                .await?;
            let embedding = self.provider.batch_embed(&pending);
            tokio::pin!(embedding);
            // A network batch can outlive the staging reaper's five-minute
            // grace period. Renew without holding a DB transaction over I/O.
            let interval = if cfg!(test) {
                std::time::Duration::from_millis(20)
            } else {
                std::time::Duration::from_secs(30)
            };
            let mut heartbeat = tokio::time::interval(interval);
            let output = loop {
                tokio::select! {
                    output = &mut embedding => break output.map_err(|error| Error::Unknown(format!("semantic embedding: {error}")))?,
                    _ = heartbeat.tick() => {
                        let updated = self.store.execute("UPDATE semanticGenerations SET touchedAt=?1 WHERE generation=?2",turso::params![chrono::Utc::now().timestamp_millis(),generation.to_owned()]).await?;
                        if updated != 1 { return Err(conflict()); }
                    }
                }
            };
            if output.len() != pending.len() {
                return Err(invalid("semantic provider returned wrong output count"));
            }
            for input in &pending {
                let output = output
                    .get(&input.id)
                    .ok_or_else(|| invalid("semantic provider omitted an input"))?;
                if output.model_signature != self.provider_signature
                    || self.provider.signature() != self.provider_signature
                {
                    return Err(invalid("semantic provider signature changed"));
                }
                vectors.insert(
                    input.id.clone(),
                    normalized(&output.vector, self.profile.dimensions)?,
                );
            }
        }
        let tx = Transaction::new_unchecked(&self.store, TransactionBehavior::Immediate).await?;
        let result=async {
            for input in &pending {
                let bytes=encode(&vectors[&input.id]); let checksum=hash(&bytes);
                self.store.execute("INSERT INTO semanticVectors(signature,inputHash,vector,checksum) VALUES(?1,?2,?3,?4) ON CONFLICT(signature,inputHash) DO NOTHING",turso::params![self.signature.clone(),input.id.clone(),bytes,checksum]).await?;
            }
            // Recheck under the writer transaction. Another worker may have
            // consumed the budget while the model request was in flight.
            semantic_limits::check("vector payload bytes per store",semantic_limits::vector_bytes(&self.store).await?,self.limits.max_vector_bytes)?;
            for chunk in chunks {
                self.store.execute("INSERT INTO semanticBindings VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",turso::params![generation.to_owned(),chunk.id.clone(),chunk.doc_id.clone(),chunk.article_id.clone(),chunk.anchor.clone(),chunk.kind.clone(),chunk.content_hash.clone(),chunk.start as i64,chunk.end as i64,chunk.input.id.clone()]).await?;
            }
            self.store.execute("UPDATE semanticGenerations SET touchedAt=?1 WHERE generation=?2",turso::params![chrono::Utc::now().timestamp_millis(),generation.to_owned()]).await?;
            Ok(())
        }.await;
        match result {
            Ok(()) => {
                tx.commit().await?;
                self.check_resources(0).await?;
                report.embedded_chunks += pending.len();
                for (id, vector) in vectors {
                    shared_vectors.entry(id).or_insert_with(|| Arc::new(vector));
                }
                Ok(chunks
                    .iter()
                    .map(|chunk| SemanticChunk {
                        chunk_id: chunk.id.clone(),
                        doc_id: chunk.doc_id.clone(),
                        article_id: chunk.article_id.clone(),
                        anchor_key: chunk.anchor.clone(),
                        kind: chunk.kind.clone(),
                        content_hash: chunk.content_hash.clone(),
                        start_byte: chunk.start,
                        end_byte: chunk.end,
                        vector: shared_vectors[&chunk.input.id].clone(),
                    })
                    .collect())
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }

    /// Failed/cancelled batches never update the head. Completed embedding
    /// batches remain cached for retry, but are not query-visible generations.
    pub async fn sync(&mut self) -> Result<(Arc<SemanticSnapshot>, SemanticSyncReport)> {
        self.sync_with_progress(false, |_| {}).await
    }

    pub async fn sync_with_progress(
        &mut self,
        force_rebuild: bool,
        mut progress: impl FnMut(SemanticProgress),
    ) -> Result<(Arc<SemanticSnapshot>, SemanticSyncReport)> {
        let mut counters = SemanticProgress {
            stage: "planning".into(),
            ..Default::default()
        };
        progress(counters.clone());
        if self.provider.signature() != self.provider_signature {
            return Err(invalid("semantic provider signature changed"));
        }
        if let Some(staging) = self.staging.take() {
            delete_unused_generation(&self.store, &staging).await?;
        }
        self.cleanup_bounded().await;
        self.check_resources(0).await?;
        let old = self.head().await?;
        let mut oversized_head = false;
        if let Some(head) = &old {
            if let Some(current) = &self.current
                && current.generation == head.generation
            {
                oversized_head = current.chunks.len() > self.limits.max_chunks;
            } else {
                let binding = self.binding_generation(&head.generation).await?;
                let mut rows = self
                    .store
                    .query(
                        "SELECT COUNT(*) FROM semanticBindings WHERE generation=?1",
                        turso::params![binding],
                    )
                    .await?;
                let count = rows
                    .next()
                    .await?
                    .ok_or_else(|| invalid("missing semantic binding count"))?
                    .get::<i64>(0)?;
                oversized_head = count > self.limits.max_chunks as i64;
            }
        }
        let force_rebuild = force_rebuild || oversized_head;
        let (revision, token, rebuild, articles) = self
            .plan(if force_rebuild { None } else { old.as_ref() })
            .await?;
        counters.target_revision = Some(revision);
        counters.target_articles = Some(articles.len());
        if !force_rebuild
            && old
                .as_ref()
                .is_some_and(|old| old.revision == revision && old.token == token)
        {
            let snapshot = match &self.current {
                Some(snapshot)
                    if Some(&snapshot.generation) == old.as_ref().map(|head| &head.generation) =>
                {
                    snapshot.clone()
                }
                _ => self
                    .snapshot()
                    .await?
                    .ok_or_else(|| invalid("semantic head disappeared"))?,
            };
            self.current = Some(snapshot.clone());
            counters.stage = "complete".into();
            progress(counters);
            return Ok((
                snapshot.clone(),
                SemanticSyncReport {
                    total_chunks: snapshot.chunks.len(),
                    ..Default::default()
                },
            ));
        }
        if !rebuild
            && let Some(old) = &old
            && let Some(result) = self.metadata_only(old, revision, &token, &articles).await?
        {
            counters.stage = "complete".into();
            counters.processed_articles = articles.len();
            counters.processed_chunks = result.1.cached_chunks;
            counters.cached_chunks = result.1.cached_chunks;
            progress(counters);
            return Ok(result);
        }
        // Carry immutable, already validated vectors into the next reader.
        // Re-decoding every unchanged vector after a metadata edit doubles the
        // vector working set and turns a one-item update into a full DB read.
        let mut next_chunks = if !rebuild {
            let base = match (&self.current, &old) {
                (Some(snapshot), Some(head)) if snapshot.generation == head.generation => {
                    Some(snapshot.clone())
                }
                _ => self.snapshot().await?,
            }
            .ok_or_else(|| invalid("semantic base generation disappeared"))?;
            let changed: HashSet<_> = articles.iter().collect();
            base.chunks
                .iter()
                .filter(|chunk| !changed.contains(&chunk.article_id))
                .cloned()
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let old_binding = match &old {
            Some(old) => Some(self.binding_generation(&old.generation).await?),
            None => None,
        };
        let mut shared_vectors = HashMap::new();
        let generation = uuid::Uuid::new_v4().to_string();
        let revision_db = i64::try_from(revision)
            .map_err(|_| invalid("semantic revision exceeds storage range"))?;
        let mut report = SemanticSyncReport {
            rebuilt: rebuild,
            changed_articles: articles.len(),
            ..Default::default()
        };
        self.staging = Some(generation.clone());
        counters.stage = "staging".into();
        progress(counters.clone());
        self.store
            .execute(
                "INSERT INTO semanticGenerations VALUES(?1,?2,?3,?4,?5,?6)",
                turso::params![
                    generation.clone(),
                    self.signature.clone(),
                    revision_db,
                    token.clone(),
                    self.profile.dimensions as i64,
                    chrono::Utc::now().timestamp_millis()
                ],
            )
            .await?;
        let result=async {
            if !rebuild && let Some(binding)=&old_binding {
                counters.stage="copying_bindings".into();progress(counters.clone());
                self.copy_bindings(binding, &generation, revision, &token).await?;
                for id in &articles {
                    self.store.execute("DELETE FROM semanticBindings WHERE generation=?1 AND articleID=?2",turso::params![generation.clone(),id.clone()]).await?;
                }
            }
            counters.stage="embedding".into();progress(counters.clone());
            for batch in articles.chunks(32) {
                let inputs=self.read_inputs(batch,revision,&token,self.limits.max_chunks.saturating_sub(next_chunks.len())).await?;
                for chunks in inputs.chunks(16) {
                    next_chunks.extend(self.append(&generation,chunks,&mut report,&mut shared_vectors).await?);
                    counters.processed_chunks+=chunks.len();
                    counters.embedded_chunks=report.embedded_chunks;
                    counters.cached_chunks=report.cached_chunks;
                    progress(counters.clone());
                }
                counters.processed_articles+=batch.len();progress(counters.clone());
            }
            counters.stage="publishing".into();progress(counters.clone());
            self.check_resources(0).await?;
            if source_version(&self.source).await?!=(revision,token.clone()) { return Err(conflict()); }
            if self.provider.signature()!=self.provider_signature {return Err(invalid("semantic provider signature changed"));}
            let tx=Transaction::new_unchecked(&self.store,TransactionBehavior::Immediate).await?;
            let publish=async {
                let current=self.head().await?;
                if current.as_ref().map(|head|&head.generation)!=old.as_ref().map(|head|&head.generation) { return Err(conflict()); }
                let mut rows=self.store.query("SELECT COUNT(*) FROM semanticBindings WHERE generation=?1",turso::params![generation.clone()]).await?;
                let count=rows.next().await?.ok_or_else(||invalid("missing semantic binding count"))?.get::<i64>(0)?;
                if count!=next_chunks.len() as i64 {return Err(invalid("semantic generation bindings changed while building"));}
                drop(rows);
                self.store.execute("INSERT INTO semanticHeads VALUES(?1,?2) ON CONFLICT(signature) DO UPDATE SET generation=excluded.generation",turso::params![self.signature.clone(),generation.clone()]).await?;
                if let Some(old)=&old {
                    delete_unused_generation(&self.store,&old.generation).await?;
                    if let Some(binding)=&old_binding {delete_unused_generation(&self.store,binding).await?;}
                }
                Ok(())
            }.await;
            match publish {Ok(())=>tx.commit().await?,Err(error)=>{let _=tx.rollback().await;return Err(error);}}
            next_chunks.sort_by(|a,b| a.chunk_id.cmp(&b.chunk_id));
            let snapshot=Arc::new(SemanticSnapshot {signature:self.signature.clone(),generation:generation.clone(),revision,token:token.clone(),dimensions:self.profile.dimensions,chunks:next_chunks});
            report.total_chunks=snapshot.chunks.len();
            counters.stage="complete".into();progress(counters.clone());
            Ok((snapshot,report.clone()))
        }.await;
        if result.is_err() {
            let _ = delete_unused_generation(&self.store, &generation).await;
            self.staging = None;
            self.cleanup_bounded().await;
        } else if let Ok((snapshot, _)) = &result {
            self.current = Some(snapshot.clone());
            self.staging = None;
            self.cleanup_bounded().await;
        }
        result
    }

    /// Copy an immutable binding set in short, keyset-paged transactions. A
    /// source update can invalidate a build between pages, and every committed
    /// page renews the staging lease. Nothing changes the published head here.
    async fn copy_bindings(&self, old: &str, new: &str, revision: u64, token: &str) -> Result<()> {
        let mut after = String::new();
        loop {
            if source_version(&self.source).await? != (revision, token.to_owned()) {
                return Err(conflict());
            }
            self.check_resources(0).await?;
            let mut rows = self.store.query(
                "SELECT chunkID FROM semanticBindings WHERE generation=?1 AND chunkID>?2 ORDER BY chunkID LIMIT 2048",
                turso::params![old.to_owned(), after.clone()],
            ).await?;
            let mut last = None;
            let mut expected = 0;
            while let Some(row) = rows.next().await? {
                last = Some(row.get::<String>(0)?);
                expected += 1;
            }
            drop(rows);
            let Some(last) = last else {
                return Ok(());
            };
            let tx =
                Transaction::new_unchecked(&self.store, TransactionBehavior::Immediate).await?;
            let result = async {
                let renewed = self.store.execute(
                    "UPDATE semanticGenerations SET touchedAt=?1 WHERE generation=?2",
                    turso::params![chrono::Utc::now().timestamp_millis(), new.to_owned()],
                ).await?;
                if renewed != 1 { return Err(conflict()); }
                // The v1 schema creates this primary-key index. Turso 0.7.2
                // otherwise selects (generation,articleID) and scans the old
                // generation again for every bounded copy page.
                let copied = self.store.execute(
                    "INSERT INTO semanticBindings SELECT ?1,chunkID,docID,articleID,anchorKey,kind,contentHash,startByte,endByte,inputHash FROM semanticBindings INDEXED BY sqlite_autoindex_semanticBindings_1 WHERE generation=?2 AND chunkID>?3 AND chunkID<=?4",
                    turso::params![new.to_owned(), old.to_owned(), after.clone(), last.clone()],
                ).await?;
                if copied != expected { return Err(conflict()); }
                Ok(())
            }.await;
            match result {
                Ok(()) => tx.commit().await?,
                Err(error) => {
                    let _ = tx.rollback().await;
                    return Err(error);
                }
            }
            after = last;
        }
    }

    pub async fn snapshot(&self) -> Result<Option<Arc<SemanticSnapshot>>> {
        self.check_resources(0).await?;
        let tx = Transaction::new_unchecked(&self.store, TransactionBehavior::Deferred).await?;
        let result=async {
            let Some(head)=self.head().await? else{return Ok(None)};
            let binding=self.binding_generation(&head.generation).await?;
            let mut count_rows=self.store.query("SELECT COUNT(*) FROM semanticBindings WHERE generation=?1",turso::params![binding.clone()]).await?;
            let count=count_rows.next().await?.ok_or_else(||invalid("missing semantic binding count"))?.get::<i64>(0)?;
            if count<0 {return Err(invalid("invalid semantic binding count"));}
            semantic_limits::check("index chunks",count as u64,self.limits.max_chunks as u64)?;
            drop(count_rows);
            // Chunk hashes are unrelated to physical vector placement. A join
            // in chunk order causes random blob reads on a cold persistent DB.
            // Resolve only required vectors through the small covering index,
            // then read their rowids in physical order, in bounded batches.
            // Rowids are transient locators inside this read transaction, never
            // persistent identities. Other model/profile blobs are not scanned.
            let mut rows=self.store.query("SELECT DISTINCT inputHash FROM semanticBindings WHERE generation=?1",turso::params![binding.clone()]).await?;
            let mut required=HashSet::<String>::new();
            while let Some(row)=rows.next().await? { required.insert(row.get(0)?); }
            drop(rows);
            let mut rows=self.store.query("SELECT rowid,inputHash FROM semanticVectors WHERE signature=?1",turso::params![self.signature.clone()]).await?;
            let mut locators=Vec::with_capacity(required.len());
            while let Some(row)=rows.next().await? {
                if required.remove(&row.get::<String>(1)?) {locators.push(row.get::<i64>(0)?);}
            }
            drop(rows);
            if !required.is_empty() {return Err(invalid("semantic generation references missing vectors"));}
            drop(required);
            locators.sort_unstable();
            let mut vectors=HashMap::with_capacity(locators.len());
            for batch in locators.chunks(256) {
                let slots=vec!["?";batch.len()].join(",");
                let sql=format!("SELECT inputHash,vector,checksum FROM semanticVectors WHERE rowid IN ({slots}) ORDER BY rowid");
                let params:Vec<Value>=batch.iter().copied().map(Into::into).collect();
                let mut rows=self.store.query(sql,params).await?;
                while let Some(row)=rows.next().await? {
                    let input:String=row.get(0)?;
                    let vector=decode(&row.get::<Vec<u8>>(1)?,&row.get::<String>(2)?,self.profile.dimensions)?;
                    vectors.insert(input,Arc::new(vector));
                }
            }
            let mut rows=self.store.query("SELECT chunkID,docID,articleID,anchorKey,kind,contentHash,startByte,endByte,inputHash FROM semanticBindings WHERE generation=?1 ORDER BY chunkID",turso::params![binding.clone()]).await?;
            let mut chunks=Vec::new();
            while let Some(row)=rows.next().await? {
                let input:String=row.get(8)?;
                let vector=Arc::clone(vectors.get(&input).ok_or_else(||invalid("semantic generation references missing vectors"))?);
                let start=row.get::<i64>(6)?;let end=row.get::<i64>(7)?;
                if start<0 || end<start {return Err(invalid("invalid semantic evidence offsets"));}
                chunks.push(SemanticChunk{chunk_id:row.get(0)?,doc_id:row.get(1)?,article_id:row.get(2)?,anchor_key:row.get(3)?,kind:row.get(4)?,content_hash:row.get(5)?,start_byte:start as usize,end_byte:end as usize,vector});
            }
            drop(rows);
            let mut rows=self.store.query("SELECT COUNT(*) FROM semanticBindings WHERE generation=?1",turso::params![binding]).await?;
            let count=rows.next().await?.ok_or_else(||invalid("missing semantic binding count"))?.get::<i64>(0)?;
            if count!=chunks.len() as i64 {return Err(invalid("semantic generation references missing vectors"));}
            Ok(Some(Arc::new(SemanticSnapshot{signature:self.signature.clone(),generation:head.generation,revision:head.revision,token:head.token,dimensions:self.profile.dimensions,chunks})))
        }.await;
        match result {
            Ok(snapshot) => {
                tx.commit().await?;
                Ok(snapshot)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }
}

fn windows(text: &str, size: Option<usize>, overlap: usize) -> Vec<(usize, usize)> {
    let Some(size) = size else {
        return vec![(0, text.len())];
    };
    let mut boundaries = text.char_indices().map(|(at, _)| at).collect::<Vec<_>>();
    boundaries.push(text.len());
    let length = boundaries.len() - 1;
    if length == 0 {
        return vec![(0, 0)];
    }
    let mut result = Vec::new();
    let mut start = 0;
    while start < length {
        let end = (start + size).min(length);
        result.push((boundaries[start], boundaries[end]));
        if end == length {
            break;
        }
        start = end - overlap;
    }
    result
}
