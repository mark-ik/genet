use malloc_size_of_derive::MallocSizeOf;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct StickyOffsetBounds {
    pub min: f32,
    pub max: f32,
}

impl StickyOffsetBounds {
    pub fn new(min: f32, max: f32) -> Self {
        Self { min, max }
    }
}
