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

/// Corner accessors for `euclid::Box2D`. Box2D stores `min` (top-left) and
/// `max` (bottom-right) corners directly; the cross corners need to be
/// constructed from those. Mirrors webrender's `LayoutRect` ergonomics
/// without pulling webrender_api back in.
pub trait BoxCorners<T, U> {
    fn top_left(&self) -> euclid::Point2D<T, U>;
    fn top_right(&self) -> euclid::Point2D<T, U>;
    fn bottom_left(&self) -> euclid::Point2D<T, U>;
    fn bottom_right(&self) -> euclid::Point2D<T, U>;
}

impl<T, U> BoxCorners<T, U> for euclid::Box2D<T, U>
where
    T: Copy,
{
    fn top_left(&self) -> euclid::Point2D<T, U> {
        self.min
    }
    fn top_right(&self) -> euclid::Point2D<T, U> {
        euclid::Point2D::new(self.max.x, self.min.y)
    }
    fn bottom_left(&self) -> euclid::Point2D<T, U> {
        euclid::Point2D::new(self.min.x, self.max.y)
    }
    fn bottom_right(&self) -> euclid::Point2D<T, U> {
        self.max
    }
}
