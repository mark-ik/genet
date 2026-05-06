use std::marker::PhantomData;

use malloc_size_of_derive::MallocSizeOf;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub struct PropertyBindingKey<T> {
    pub id: u64,
    #[serde(skip)]
    marker: PhantomData<T>,
}

impl<T> PropertyBindingKey<T> {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            marker: PhantomData,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub enum PropertyValue<T> {
    Value(T),
    Binding(PropertyBindingKey<T>, T),
}
