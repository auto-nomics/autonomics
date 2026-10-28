//! Boolean/article evaluation needs every scored match, not a sorted Top-K.
//! Sorting each term/field before merging wastes work; the final article list
//! receives its deterministic ordering after aggregation and hard filtering.
use tantivy::{
    DocAddress, DocId, Score, SegmentOrdinal, SegmentReader,
    collector::{Collector, SegmentCollector},
};

pub(super) struct AllScoredDocs;
pub(super) struct ScoredSegment {
    ordinal: SegmentOrdinal,
    hits: Vec<(Score, DocAddress)>,
}
impl Collector for AllScoredDocs {
    type Fruit = Vec<(Score, DocAddress)>;
    type Child = ScoredSegment;
    fn for_segment(
        &self,
        ordinal: SegmentOrdinal,
        _: &SegmentReader,
    ) -> tantivy::Result<Self::Child> {
        Ok(ScoredSegment {
            ordinal,
            hits: Vec::new(),
        })
    }
    fn requires_scoring(&self) -> bool {
        true
    }
    fn merge_fruits(&self, fruits: Vec<Self::Fruit>) -> tantivy::Result<Self::Fruit> {
        Ok(fruits.into_iter().flatten().collect())
    }
}
impl SegmentCollector for ScoredSegment {
    type Fruit = Vec<(Score, DocAddress)>;
    fn collect(&mut self, doc_id: DocId, score: Score) {
        self.hits.push((
            score,
            DocAddress {
                segment_ord: self.ordinal,
                doc_id,
            },
        ));
    }
    fn harvest(self) -> Self::Fruit {
        self.hits
    }
}
