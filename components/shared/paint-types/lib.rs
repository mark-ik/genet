use std::hash::{Hash, Hasher};
use std::ops::{BitOr, BitOrAssign};

use malloc_size_of_derive::MallocSizeOf;
use serde::{Deserialize, Serialize};

pub mod border;
pub mod color;
pub mod composite;
pub mod gradient;
pub mod ids;
pub mod image;
pub mod property;
pub mod sticky;
pub mod units;

pub use border::{BorderRadius, BorderStyle, BoxShadowClipMode, LineStyle, NormalBorder};
pub use color::ColorF;
pub use composite::{ImageRendering, MixBlendMode, TransformStyle};
pub use gradient::{ExtendMode, GradientStop, ReferenceFrameKind, RepeatMode};
pub use ids::{
    DocumentId, Epoch, ExternalImageId, ExternalScrollId, FontInstanceKey, FontKey, IdNamespace,
    ImageKey, PipelineId, SpatialId, SpatialTreeItemKey,
};
pub use image::{
    BuiltDisplayList, BuiltDisplayListData, BuiltDisplayListDescriptor, ExternalImage,
    ExternalImageData, ExternalImageHandler, ExternalImageSource, ExternalImageType,
    ImageBufferKind, ImageData, ImageDescriptor, ImageDescriptorFlags, ImageFormat,
    NativeFontHandle, SerializableImageData,
};
pub use property::{PropertyBindingKey, PropertyValue};
pub use sticky::StickyOffsetBounds;

#[derive(Clone, Copy, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct FontVariation {
    pub tag: u32,
    pub value: f32,
}

impl Eq for FontVariation {}

impl Hash for FontVariation {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.tag.hash(state);
        self.value.to_bits().hash(state);
    }
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub struct FontInstanceFlags(pub u32);

impl FontInstanceFlags {
    pub const SYNTHETIC_BOLD: Self = Self(1 << 1);
    pub const EMBEDDED_BITMAPS: Self = Self(1 << 2);
    pub const SUBPIXEL_POSITION: Self = Self(1 << 3);

    pub fn empty() -> Self {
        Self(0)
    }
}

impl BitOr for FontInstanceFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for FontInstanceFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[derive(Clone, Copy, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub enum ScrollLocation {
    Delta(units::LayoutVector2D),
    Start,
    End,
}
