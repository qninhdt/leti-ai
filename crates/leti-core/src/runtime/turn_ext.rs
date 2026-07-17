//! Opaque, runtime-only values supplied by the host for one turn.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

type Value = Arc<dyn Any + Send + Sync>;

/// Typed extensions that flow through one turn and its tool calls.
///
/// The engine only transports this carrier. It never interprets, persists,
/// serializes, or logs the values stored in it.
#[derive(Clone, Default)]
pub struct TurnExtensions(Arc<HashMap<TypeId, Value>>);

impl fmt::Debug for TurnExtensions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TurnExtensions")
            .field("value_count", &self.0.len())
            .finish()
    }
}

impl TurnExtensions {
    /// Return a new carrier with a typed value added or replaced.
    #[must_use]
    pub fn with<T>(&self, value: T) -> Self
    where
        T: Any + Send + Sync,
    {
        let mut values = self.0.as_ref().clone();
        values.insert(TypeId::of::<T>(), Arc::new(value));
        Self(Arc::new(values))
    }

    /// Downcast a host-owned value by its concrete type.
    #[must_use]
    pub fn get<T>(&self) -> Option<&T>
    where
        T: Any + Send + Sync,
    {
        self.0
            .get(&TypeId::of::<T>())
            .and_then(|value| value.downcast_ref::<T>())
    }
}
