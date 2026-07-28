//! Intrinsic-size queries and their box-keyed cache.

use std::collections::{HashMap, HashSet};

use crate::{BoxId, LogicalAxis};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum IntrinsicSizeKind {
    MinContent,
    MaxContent,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct IntrinsicSizeQuery {
    pub box_id: BoxId,
    pub axis: LogicalAxis,
    pub kind: IntrinsicSizeKind,
}

impl IntrinsicSizeQuery {
    pub const fn new(box_id: BoxId, axis: LogicalAxis, kind: IntrinsicSizeKind) -> Self {
        Self { box_id, axis, kind }
    }
}

/// A standards-visible reason an intrinsic query has no answer in this lane.
///
/// Callers must preserve these outcomes instead of replacing them with an
/// automatic size, zero, or a completed used size.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntrinsicQueryError {
    Cycle(IntrinsicSizeQuery),
    IndefiniteContainingSize(IntrinsicSizeQuery),
    FragmentationDependent(IntrinsicSizeQuery),
    UnsupportedAxis(IntrinsicSizeQuery),
}

/// Whether an intrinsic query was already cached or now owns the right to
/// compute its pair of min/max values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IntrinsicQueryState {
    Cached(f32),
    Pending,
}

/// Both intrinsic sizes for one box and logical axis.
///
/// Computing the pair together lets an inline formatting context shape its
/// minimum and maximum content cases once, then answer either CSS query.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IntrinsicSizes {
    pub min_content: f32,
    pub max_content: f32,
}

impl IntrinsicSizes {
    pub fn new(min_content: f32, max_content: f32) -> Option<Self> {
        (min_content.is_finite()
            && max_content.is_finite()
            && min_content >= 0.0
            && max_content >= min_content)
            .then_some(Self {
                min_content,
                max_content,
            })
    }

    pub const fn get(self, kind: IntrinsicSizeKind) -> f32 {
        match kind {
            IntrinsicSizeKind::MinContent => self.min_content,
            IntrinsicSizeKind::MaxContent => self.max_content,
        }
    }
}

/// Results cached by standards-owned box identity rather than backend nodes.
#[derive(Clone, Debug, Default)]
pub struct IntrinsicSizeCache {
    entries: HashMap<(BoxId, LogicalAxis), IntrinsicSizes>,
    pending: HashSet<(BoxId, LogicalAxis)>,
}

impl IntrinsicSizeCache {
    pub fn get(&self, query: IntrinsicSizeQuery) -> Option<f32> {
        self.entries
            .get(&(query.box_id, query.axis))
            .copied()
            .map(|sizes| sizes.get(query.kind))
    }

    pub fn insert(&mut self, box_id: BoxId, axis: LogicalAxis, sizes: IntrinsicSizes) {
        self.entries.insert((box_id, axis), sizes);
    }

    /// Begin a query without conflating a re-entrant request with a usable
    /// value. A provider that needs another intrinsic answer can reserve it
    /// first and report a cycle explicitly.
    pub fn begin_query(
        &mut self,
        query: IntrinsicSizeQuery,
    ) -> Result<IntrinsicQueryState, IntrinsicQueryError> {
        if let Some(size) = self.get(query) {
            return Ok(IntrinsicQueryState::Cached(size));
        }
        let key = (query.box_id, query.axis);
        if !self.pending.insert(key) {
            return Err(IntrinsicQueryError::Cycle(query));
        }
        Ok(IntrinsicQueryState::Pending)
    }

    /// Finish a query reserved with [`Self::begin_query`]. Deferred outcomes
    /// are intentionally not cached, so a later layout pass can provide a
    /// newly definite basis without inheriting a stale failure.
    pub fn finish_query(
        &mut self,
        query: IntrinsicSizeQuery,
        sizes: Result<IntrinsicSizes, IntrinsicQueryError>,
    ) -> Result<f32, IntrinsicQueryError> {
        self.pending.remove(&(query.box_id, query.axis));
        let sizes = sizes?;
        let result = sizes.get(query.kind);
        self.insert(query.box_id, query.axis, sizes);
        Ok(result)
    }

    pub fn query_with<Error>(
        &mut self,
        query: IntrinsicSizeQuery,
        compute: impl FnOnce(BoxId, LogicalAxis) -> Result<IntrinsicSizes, Error>,
    ) -> Result<f32, Error> {
        if let Some(size) = self.get(query) {
            return Ok(size);
        }
        let sizes = compute(query.box_id, query.axis)?;
        let result = sizes.get(query.kind);
        self.insert(query.box_id, query.axis, sizes);
        Ok(result)
    }

    pub fn invalidate(&mut self, box_id: BoxId) {
        self.entries
            .retain(|(candidate, _), _| *candidate != box_id);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.pending.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Compute a block-axis intrinsic pair only when an unfragmented formatting
/// context has a definite inline basis. Under that condition both CSS
/// min-content and max-content block queries describe the same formatting
/// result; wrapping-dependent or fragmentainer-dependent answers stay out of
/// this lane.
pub fn block_intrinsic_sizes_for_definite_inline(
    query: IntrinsicSizeQuery,
    containing_inline_size: Option<f32>,
    fragmented: bool,
    measure_block_size: impl FnOnce(f32) -> Option<f32>,
) -> Result<IntrinsicSizes, IntrinsicQueryError> {
    if query.axis != LogicalAxis::Block {
        return Err(IntrinsicQueryError::UnsupportedAxis(query));
    }
    if fragmented {
        return Err(IntrinsicQueryError::FragmentationDependent(query));
    }
    let Some(containing_inline_size) =
        containing_inline_size.filter(|size| size.is_finite() && *size >= 0.0)
    else {
        return Err(IntrinsicQueryError::IndefiniteContainingSize(query));
    };
    let Some(block_size) =
        measure_block_size(containing_inline_size).filter(|size| size.is_finite() && *size >= 0.0)
    else {
        return Err(IntrinsicQueryError::IndefiniteContainingSize(query));
    };
    IntrinsicSizes::new(block_size, block_size)
        .ok_or(IntrinsicQueryError::IndefiniteContainingSize(query))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BoxOrigin, ContainingBlock, CssBox, CssBoxTree, DisplayRole, FlowAxes, PositioningScheme,
    };

    #[test]
    fn min_and_max_content_are_distinct_queries_with_one_cached_measurement() {
        let mut boxes = CssBoxTree::default();
        let box_id = boxes.push(
            CssBox::new(
                BoxOrigin::Element(1u8),
                DisplayRole::INLINE_FLOW,
                FlowAxes::HORIZONTAL_LTR,
                PositioningScheme::Static,
                false,
                None,
                ContainingBlock::Initial,
            ),
            None,
            true,
        );
        let mut cache = IntrinsicSizeCache::default();
        let mut measurements = 0;
        let mut query = |kind| {
            cache
                .query_with(
                    IntrinsicSizeQuery::new(box_id, LogicalAxis::Inline, kind),
                    |_, _| {
                        measurements += 1;
                        Ok::<_, ()>(IntrinsicSizes::new(40.0, 120.0).expect("valid sizes"))
                    },
                )
                .expect("infallible measurement")
        };

        assert_eq!(query(IntrinsicSizeKind::MinContent), 40.0);
        assert_eq!(query(IntrinsicSizeKind::MaxContent), 120.0);
        assert_eq!(measurements, 1);
        assert_eq!(cache.len(), 1);

        cache.invalidate(box_id);
        assert!(cache.is_empty());
    }

    #[test]
    fn invalid_intrinsic_pairs_are_rejected() {
        assert_eq!(IntrinsicSizes::new(f32::NAN, 10.0), None);
        assert_eq!(IntrinsicSizes::new(-1.0, 10.0), None);
        assert_eq!(IntrinsicSizes::new(20.0, 10.0), None);
    }

    #[test]
    fn block_queries_are_distinct_from_inline_queries_and_require_a_definite_basis() {
        let mut boxes = CssBoxTree::default();
        let box_id = boxes.push(
            CssBox::new(
                BoxOrigin::Element(1u8),
                DisplayRole::BLOCK_FLOW,
                FlowAxes::HORIZONTAL_LTR,
                PositioningScheme::Static,
                false,
                None,
                ContainingBlock::Initial,
            ),
            None,
            true,
        );
        let inline = IntrinsicSizes::new(40.0, 120.0).expect("valid inline sizes");
        let min_block =
            IntrinsicSizeQuery::new(box_id, LogicalAxis::Block, IntrinsicSizeKind::MinContent);
        let max_block =
            IntrinsicSizeQuery::new(box_id, LogicalAxis::Block, IntrinsicSizeKind::MaxContent);
        let block =
            block_intrinsic_sizes_for_definite_inline(min_block, Some(80.0), false, |width| {
                Some(width / 4.0)
            })
            .expect("unfragmented definite block query");
        let mut cache = IntrinsicSizeCache::default();
        cache.insert(box_id, LogicalAxis::Inline, inline);
        cache.insert(box_id, LogicalAxis::Block, block);

        assert_eq!(
            cache.get(IntrinsicSizeQuery::new(
                box_id,
                LogicalAxis::Inline,
                IntrinsicSizeKind::MinContent
            )),
            Some(40.0)
        );
        assert_eq!(
            cache.get(IntrinsicSizeQuery::new(
                box_id,
                LogicalAxis::Inline,
                IntrinsicSizeKind::MaxContent
            )),
            Some(120.0)
        );
        assert_eq!(cache.get(min_block), Some(20.0));
        assert_eq!(cache.get(max_block), Some(20.0));
        assert_eq!(
            block_intrinsic_sizes_for_definite_inline(min_block, None, false, |_| Some(0.0)),
            Err(IntrinsicQueryError::IndefiniteContainingSize(min_block))
        );
        assert_eq!(
            block_intrinsic_sizes_for_definite_inline(min_block, Some(80.0), true, |_| Some(20.0)),
            Err(IntrinsicQueryError::FragmentationDependent(min_block))
        );
    }

    #[test]
    fn query_cycles_are_explicit_and_do_not_poison_the_cache() {
        let mut boxes = CssBoxTree::default();
        let box_id = boxes.push(
            CssBox::new(
                BoxOrigin::Element(1u8),
                DisplayRole::BLOCK_FLOW,
                FlowAxes::HORIZONTAL_LTR,
                PositioningScheme::Static,
                false,
                None,
                ContainingBlock::Initial,
            ),
            None,
            true,
        );
        let query =
            IntrinsicSizeQuery::new(box_id, LogicalAxis::Block, IntrinsicSizeKind::MinContent);
        let mut cache = IntrinsicSizeCache::default();

        assert_eq!(cache.begin_query(query), Ok(IntrinsicQueryState::Pending));
        assert_eq!(
            cache.begin_query(query),
            Err(IntrinsicQueryError::Cycle(query))
        );
        assert_eq!(
            cache.finish_query(
                query,
                Err(IntrinsicQueryError::IndefiniteContainingSize(query))
            ),
            Err(IntrinsicQueryError::IndefiniteContainingSize(query))
        );
        assert_eq!(cache.get(query), None);
        assert_eq!(cache.begin_query(query), Ok(IntrinsicQueryState::Pending));
    }
}
