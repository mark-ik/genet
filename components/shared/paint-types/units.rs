pub use app_units::Au;

pub enum DevicePixel {}
pub enum FramebufferPixel {}
pub enum LayoutPixel {}
pub enum PicturePixel {}
pub enum RasterPixel {}
pub enum TexelPixel {}
pub enum WorldPixel {}

pub type DeviceIntPoint = euclid::Point2D<i32, DevicePixel>;
pub type DeviceIntRect = euclid::Box2D<i32, DevicePixel>;
pub type DeviceIntSize = euclid::Size2D<i32, DevicePixel>;
pub type DeviceIntSideOffsets = euclid::SideOffsets2D<i32, DevicePixel>;
pub type DeviceIntVector2D = euclid::Vector2D<i32, DevicePixel>;
pub type DevicePoint = euclid::Point2D<f32, DevicePixel>;
pub type DeviceRect = euclid::Box2D<f32, DevicePixel>;
pub type DeviceSize = euclid::Size2D<f32, DevicePixel>;
pub type DeviceVector2D = euclid::Vector2D<f32, DevicePixel>;

pub type LayoutPoint = euclid::Point2D<f32, LayoutPixel>;
pub type LayoutRect = euclid::Box2D<f32, LayoutPixel>;
pub type LayoutSideOffsets = euclid::SideOffsets2D<f32, LayoutPixel>;
pub type LayoutSize = euclid::Size2D<f32, LayoutPixel>;
pub type LayoutTransform = euclid::Transform3D<f32, LayoutPixel, LayoutPixel>;
pub type LayoutVector2D = euclid::Vector2D<f32, LayoutPixel>;

pub type PictureRect = euclid::Box2D<f32, PicturePixel>;
pub type RasterRect = euclid::Box2D<f32, RasterPixel>;
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    malloc_size_of_derive::MallocSizeOf,
    PartialEq,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct TexelRect {
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

impl TexelRect {
    pub fn new(u0: f32, v0: f32, u1: f32, v1: f32) -> Self {
        Self { u0, v0, u1, v1 }
    }
}
pub type WorldPoint = euclid::Point2D<f32, WorldPixel>;
pub type WorldRect = euclid::Box2D<f32, WorldPixel>;
pub type WorldVector2D = euclid::Vector2D<f32, WorldPixel>;

pub trait RectExt<T, U> {
    fn is_well_formed_and_nonempty(&self) -> bool;
}

impl<T, U> RectExt<T, U> for euclid::Rect<T, U>
where
    T: Copy + PartialOrd + Default,
{
    fn is_well_formed_and_nonempty(&self) -> bool {
        self.size.width > T::default() && self.size.height > T::default()
    }
}
