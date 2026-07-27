//! Intrinsic-size queries and their box-keyed cache.

use std::collections::HashMap;

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
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
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
}
