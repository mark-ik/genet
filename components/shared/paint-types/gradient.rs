use malloc_size_of_derive::MallocSizeOf;
use serde::{Deserialize, Serialize};

use crate::color::ColorF;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub enum ExtendMode {
    Clamp,
    Repeat,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub enum RepeatMode {
    Stretch,
    Repeat,
    Round,
    Space,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub enum ReferenceFrameKind {
    Transform,
    Perspective,
}

impl Default for ReferenceFrameKind {
    fn default() -> Self {
        Self::Transform
    }
}

#[derive(Clone, Copy, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct GradientStop {
    pub offset: f32,
    pub color: ColorF,
}
