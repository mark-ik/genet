use malloc_size_of_derive::MallocSizeOf;
use serde::{Deserialize, Serialize};

use crate::color::ColorF;
use crate::units::{LayoutSideOffsets, LayoutSize};

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub enum BorderStyle {
    #[default]
    None,
    Solid,
    Double,
    Dotted,
    Dashed,
    Hidden,
    Groove,
    Ridge,
    Inset,
    Outset,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub enum LineStyle {
    Solid,
    Dotted,
    Dashed,
    Wavy,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub enum BoxShadowClipMode {
    Outset,
    Inset,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct BorderRadius {
    pub top_left: LayoutSize,
    pub top_right: LayoutSize,
    pub bottom_left: LayoutSize,
    pub bottom_right: LayoutSize,
}

impl BorderRadius {
    pub fn zero() -> Self {
        Self::default()
    }

    pub fn is_zero(&self) -> bool {
        let zero = LayoutSize::zero();
        self.top_left == zero
            && self.top_right == zero
            && self.bottom_left == zero
            && self.bottom_right == zero
    }
}

#[derive(Clone, Copy, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct NormalBorder {
    pub left: BorderSide,
    pub right: BorderSide,
    pub top: BorderSide,
    pub bottom: BorderSide,
    pub radius: BorderRadius,
    pub do_aa: bool,
}

impl Default for NormalBorder {
    fn default() -> Self {
        Self {
            left: BorderSide::default(),
            right: BorderSide::default(),
            top: BorderSide::default(),
            bottom: BorderSide::default(),
            radius: BorderRadius::default(),
            do_aa: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct BorderSide {
    pub color: ColorF,
    pub style: BorderStyle,
}

impl Default for BorderSide {
    fn default() -> Self {
        Self {
            color: ColorF::TRANSPARENT,
            style: BorderStyle::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct BorderWidths(pub LayoutSideOffsets);
