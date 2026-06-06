use malloc_size_of_derive::MallocSizeOf;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::ids::ExternalImageId;
use crate::units::{DeviceIntSize, TexelRect};

#[derive(Clone, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct BuiltDisplayListDescriptor;

#[derive(Clone, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct BuiltDisplayListData {
    pub items_data: Vec<u8>,
    pub spatial_tree: Vec<u8>,
}

#[derive(Clone, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct BuiltDisplayList {
    pub data: BuiltDisplayListData,
    pub descriptor: BuiltDisplayListDescriptor,
}

impl BuiltDisplayList {
    pub fn into_data(self) -> (BuiltDisplayListData, BuiltDisplayListDescriptor) {
        (self.data, self.descriptor)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub enum ImageFormat {
    R8,
    R16,
    RG8,
    RGBA8,
    BGRA8,
    RGBAF32,
}

impl ImageFormat {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::R8 => 1,
            Self::R16 | Self::RG8 => 2,
            Self::RGBA8 | Self::BGRA8 => 4,
            Self::RGBAF32 => 16,
        }
    }
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub struct ImageDescriptorFlags(pub u32);

impl ImageDescriptorFlags {
    pub const IS_OPAQUE: Self = Self(1 << 0);
    pub const ALLOW_MIPMAPS: Self = Self(1 << 1);

    pub fn empty() -> Self {
        Self(0)
    }

    pub fn set(&mut self, flag: Self, enabled: bool) {
        if enabled {
            self.0 |= flag.0;
        } else {
            self.0 &= !flag.0;
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct ImageDescriptor {
    pub size: DeviceIntSize,
    pub stride: Option<i32>,
    pub format: ImageFormat,
    pub offset: i32,
    pub flags: ImageDescriptorFlags,
}

impl ImageDescriptor {
    pub fn new(width: i32, height: i32, format: ImageFormat, flags: ImageDescriptorFlags) -> Self {
        Self {
            size: DeviceIntSize::new(width, height),
            stride: None,
            format,
            offset: 0,
            flags,
        }
    }
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub enum ImageData {
    Raw(Vec<u8>),
    External(ExternalImageData),
}

impl ImageData {
    pub fn new(data: Vec<u8>) -> Self {
        Self::Raw(data)
    }
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct SerializableImageData {
    pub descriptor: ImageDescriptor,
    pub data: ImageData,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub struct ExternalImageData {
    pub id: ExternalImageId,
    pub channel_index: u8,
    pub image_type: ExternalImageType,
    pub normalized_uvs: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub enum ExternalImageType {
    TextureHandle(ImageBufferKind),
    Buffer,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub enum ImageBufferKind {
    Texture2D,
    TextureRect,
    TextureExternal,
    Buffer,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct ExternalImage {
    pub uv: TexelRect,
    pub source: ExternalImageSource,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub enum ExternalImageSource {
    NativeTexture(u32),
    RawData(Vec<u8>),
    Invalid,
}

pub trait ExternalImageHandler {
    fn lock(
        &mut self,
        key: ExternalImageId,
        channel_index: u8,
        is_composited: bool,
    ) -> ExternalImage;

    fn unlock(&mut self, key: ExternalImageId, channel_index: u8);
}

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_os = "windows"
))]
#[derive(Clone, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub struct NativeFontHandle {
    pub path: PathBuf,
    pub index: u32,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub struct NativeFontHandle {
    pub name: String,
    pub path: String,
}

// Other targets (notably wasm32-unknown-unknown) have no native font handle;
// fonts come from the host (parley's system fonts / the browser). A unit
// placeholder so the type and its re-export exist. (wasm de-IPC pass, 2026-06-06.)
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_os = "windows",
    target_os = "macos"
)))]
#[derive(Clone, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub struct NativeFontHandle;
