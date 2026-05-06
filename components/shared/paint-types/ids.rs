use malloc_size_of_derive::MallocSizeOf;
use serde::{Deserialize, Serialize};

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Deserialize,
    Eq,
    Hash,
    MallocSizeOf,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
)]
pub struct IdNamespace(pub u32);

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub struct ImageKey(pub IdNamespace, pub u32);

impl ImageKey {
    pub fn new(namespace: IdNamespace, key: u32) -> Self {
        Self(namespace, key)
    }
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub struct FontKey(pub IdNamespace, pub u32);

impl FontKey {
    pub fn new(namespace: IdNamespace, key: u32) -> Self {
        Self(namespace, key)
    }
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub struct FontInstanceKey(pub IdNamespace, pub u32);

impl FontInstanceKey {
    pub fn new(namespace: IdNamespace, key: u32) -> Self {
        Self(namespace, key)
    }
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub struct PipelineId(pub u32, pub u32);

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub struct DocumentId(pub u32, pub u32);

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub struct Epoch(pub u32);

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub struct ExternalImageId(pub u64);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub struct ExternalScrollId(pub u64, pub PipelineId);

impl Default for ExternalScrollId {
    fn default() -> Self {
        Self(0, PipelineId::default())
    }
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub struct SpatialId(pub u64, pub PipelineId);

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub struct SpatialTreeItemKey(pub u64, pub u64);

impl SpatialTreeItemKey {
    pub fn new(namespace: u64, key: u64) -> Self {
        Self(namespace, key)
    }
}
