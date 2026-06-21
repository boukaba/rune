use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// A property key (interned string index or symbol).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct PropertyKey(u64);

/// An immutable shape — maps property keys to slot offsets.
/// Shapes are hash-consed globally; each unique entry list maps to exactly one `&'static Shape`.
pub struct Shape {
    pub id: u64,
    pub property_count: usize,
    pub slot_count: usize,
    pub entries: Vec<(PropertyKey, usize)>,
    /// Original property key names for for-in enumeration (same order as entries).
    pub key_names: Vec<String>,
    pub parent: Option<u64>,
    pub is_dense_array: bool,
}

static SHAPE_COUNTER: AtomicU64 = AtomicU64::new(1);

lazy_static::lazy_static! {
    static ref SHAPE_TABLE: Mutex<HashMap<Vec<(PropertyKey, usize)>, &'static Shape>> =
        Mutex::new(HashMap::new());
    /// Interned PropertyKey for "prototype" — avoids HeapString alloc on every `new` call.
    pub static ref PROTOTYPE_KEY: PropertyKey = PropertyKey::from_string("prototype");
    /// Shared shape for all dense arrays.
    pub static ref DENSE_ARRAY_SHAPE: &'static Shape = {
        let id = SHAPE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let shape = Box::new(Shape {
            id,
            property_count: 0,
            slot_count: 0,
            entries: Vec::new(),
            key_names: Vec::new(),
            parent: None,
            is_dense_array: true,
        });
        Box::leak(shape)
    };
}

impl Shape {
    /// Create a new shape with the given entries and intern it globally.
    /// Returns a `&'static Shape` that lives for the program's lifetime.
    pub fn intern(entries: Vec<(PropertyKey, usize)>, key_names: Vec<String>) -> &'static Self {
        let mut table = SHAPE_TABLE.lock().unwrap();
        if let Some(existing) = table.get(&entries) {
            return existing;
        }
        let slot_count = entries.len();
        let id = SHAPE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let shape = Shape {
            id,
            property_count: entries.len(),
            slot_count,
            entries: entries.clone(),
            key_names,
            parent: None,
            is_dense_array: false,
        };
        let leaked: &'static Shape = Box::leak(Box::new(shape));
        table.insert(entries, leaked);
        leaked
    }

    /// Intern a shape that extends a parent shape with one additional property.
    /// The new property gets the next slot offset.
    pub fn intern_with_parent(parent: &Self, key: PropertyKey, key_name: String) -> &'static Self {
        let mut entries = parent.entries.clone();
        let offset = entries.len();
        entries.push((key, offset));
        let mut key_names = parent.key_names.clone();
        key_names.push(key_name);
        Self::intern(entries, key_names)
    }

    /// Convenience: intern an empty shape.
    pub fn empty() -> &'static Self {
        Self::intern(vec![], vec![])
    }

    /// Create a new shape (for tests or temporary use).
    /// Prefer `intern()` in production code.
    pub fn new(entries: Vec<(PropertyKey, usize)>, key_names: Vec<String>) -> Box<Self> {
        let id = SHAPE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let slot_count = entries.len();
        Box::new(Shape {
            id,
            property_count: entries.len(),
            slot_count,
            entries,
            key_names,
            parent: None,
            is_dense_array: false,
        })
    }

    pub fn lookup(&self, key: &PropertyKey) -> Option<usize> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, offset)| *offset)
    }

    /// Return the key name at the given entry index (for for-in enumeration).
    pub fn key_name_at(&self, index: usize) -> Option<&str> {
        self.key_names.get(index).map(|s| s.as_str())
    }
}

impl PropertyKey {
    pub fn from_string(s: &str) -> Self {
        let hash = fxhash::hash64(s.as_bytes());
        PropertyKey(hash)
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl std::fmt::Debug for Shape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shape")
            .field("id", &self.id)
            .field("entry_count", &self.property_count)
            .finish()
    }
}
