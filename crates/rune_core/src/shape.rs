use std::collections::HashMap;
use std::sync::Mutex;

/// A property key (interned string index or symbol).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct PropertyKey(u64);

/// Per-property attributes (§6.1.7.1 — the data-descriptor flags; accessor
/// vs data is structural via TAG_ACCESSOR slot values, not stored here).
/// F5 reserves the home + identity; A4 reads/writes them.
pub type PropAttr = u8;
/// Default attributes: writable + enumerable + configurable.
pub const ATTR_DEFAULT: PropAttr = 0b111;
pub const ATTR_WRITABLE: PropAttr = 0b001;
pub const ATTR_ENUMERABLE: PropAttr = 0b010;
pub const ATTR_CONFIGURABLE: PropAttr = 0b100;

/// An immutable shape — maps property keys to slot offsets.
/// Shapes are hash-consed globally; each unique (entries, attrs) list maps
/// to exactly one `&'static Shape`.
#[repr(C)]
pub struct Shape {
    pub id: u64,
    pub property_count: usize,
    pub slot_count: usize,
    pub entries: Vec<(PropertyKey, usize)>,
    /// Attribute byte per entry (parallel to `entries`; see PropAttr).
    /// All-ATTR_DEFAULT until A4's defineProperty writes non-defaults.
    pub attrs: Vec<PropAttr>,
    /// Original property key names for for-in enumeration (same order as entries).
    pub key_names: Vec<String>,
    pub parent: Option<u64>,
    pub is_dense_array: bool,
}

/// Compute a stable, content-addressed shape id from its defining data.
/// This makes shape ids deterministic across process restarts, which is
/// required for AFPC native-code caches to remain valid after load.
/// F5: attributes are part of the identity — same keys/offsets with
/// different attributes are DIFFERENT shapes, so IC (shape.id → slot)
/// caches stay sound when A4 introduces non-default attributes.
fn shape_id(
    entries: &[(PropertyKey, usize)],
    attrs: &[PropAttr],
    parent: Option<u64>,
    is_dense_array: bool,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = fxhash::FxHasher64::default();
    for (key, offset) in entries {
        key.as_u64().hash(&mut hasher);
        (*offset as u64).hash(&mut hasher);
    }
    for attr in attrs {
        (*attr as u64).hash(&mut hasher);
    }
    parent.hash(&mut hasher);
    is_dense_array.hash(&mut hasher);
    hasher.finish()
}

/// Hash-consing key: (entries, attrs). Attributes are part of the key so
/// same-keys/different-attrs shapes never collide (see `shape_id`).
type ShapeTableKey = (Vec<(PropertyKey, usize)>, Vec<PropAttr>);

lazy_static::lazy_static! {
    static ref SHAPE_TABLE: Mutex<HashMap<ShapeTableKey, &'static Shape>> =
        Mutex::new(HashMap::new());
    /// Interned PropertyKey for "prototype" — avoids HeapString alloc on every `new` call.
    pub static ref PROTOTYPE_KEY: PropertyKey = PropertyKey::from_string("prototype");
    /// Shared shape for all dense arrays.
    pub static ref DENSE_ARRAY_SHAPE: &'static Shape = {
        let entries: Vec<(PropertyKey, usize)> = Vec::new();
        let id = shape_id(&entries, &[], None, true);
        let shape = Box::new(Shape {
            id,
            property_count: 0,
            slot_count: 0,
            entries,
            attrs: Vec::new(),
            key_names: Vec::new(),
            parent: None,
            is_dense_array: true,
        });
        Box::leak(shape)
    };
}

impl Shape {
    /// Create a new shape with the given entries and intern it globally.
    /// Attributes default to ATTR_DEFAULT for every entry (all current
    /// producers); use `intern_with_attrs` for non-default attributes.
    /// Returns a `&'static Shape` that lives for the program's lifetime.
    pub fn intern(entries: Vec<(PropertyKey, usize)>, key_names: Vec<String>) -> &'static Self {
        let attrs = vec![ATTR_DEFAULT; entries.len()];
        Self::intern_with_attrs(entries, key_names, attrs)
    }

    /// Intern a shape with explicit per-entry attributes (parallel to
    /// `entries`; short vectors are padded with ATTR_DEFAULT).
    pub fn intern_with_attrs(
        entries: Vec<(PropertyKey, usize)>,
        key_names: Vec<String>,
        mut attrs: Vec<PropAttr>,
    ) -> &'static Self {
        attrs.resize(entries.len(), ATTR_DEFAULT);
        let mut table = SHAPE_TABLE.lock().unwrap();
        let key = (entries.clone(), attrs.clone());
        if let Some(existing) = table.get(&key) {
            return existing;
        }
        let slot_count = entries.len();
        let id = shape_id(&entries, &attrs, None, false);
        let shape = Shape {
            id,
            property_count: entries.len(),
            slot_count,
            entries: entries.clone(),
            attrs: attrs.clone(),
            key_names,
            parent: None,
            is_dense_array: false,
        };
        let leaked: &'static Shape = Box::leak(Box::new(shape));
        table.insert(key, leaked);
        leaked
    }

    /// Intern a shape that extends a parent shape with one additional property.
    /// The new property gets the next slot offset with default attributes.
    pub fn intern_with_parent(parent: &Self, key: PropertyKey, key_name: String) -> &'static Self {
        let mut entries = parent.entries.clone();
        let offset = entries.len();
        entries.push((key, offset));
        let mut key_names = parent.key_names.clone();
        key_names.push(key_name);
        let mut attrs = parent.attrs.clone();
        attrs.push(ATTR_DEFAULT);
        Self::intern_with_attrs(entries, key_names, attrs)
    }

    /// Convenience: intern an empty shape.
    pub fn empty() -> &'static Self {
        Self::intern(vec![], vec![])
    }

    /// Create a new shape (for tests or temporary use).
    /// Prefer `intern()` in production code.
    pub fn new(entries: Vec<(PropertyKey, usize)>, key_names: Vec<String>) -> Box<Self> {
        let attrs = vec![ATTR_DEFAULT; entries.len()];
        let id = shape_id(&entries, &attrs, None, false);
        let slot_count = entries.len();
        Box::new(Shape {
            id,
            property_count: entries.len(),
            slot_count,
            entries,
            attrs,
            key_names,
            parent: None,
            is_dense_array: false,
        })
    }

    /// Attribute byte for the entry at `index` (ATTR_DEFAULT when absent —
    /// defensive for hand-built shapes; interned shapes always carry one).
    pub fn attr_at(&self, index: usize) -> PropAttr {
        self.attrs.get(index).copied().unwrap_or(ATTR_DEFAULT)
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

/// Snapshot all currently-interned shapes. Used by AFPC to persist the
/// global shape table so cached native code remains valid across runs.
pub fn snapshot_shapes() -> Vec<&'static Shape> {
    let table = SHAPE_TABLE.lock().unwrap();
    table.values().copied().collect()
}

impl PropertyKey {
    pub fn from_string(s: &str) -> Self {
        // Clear the symbol-flag bit so string keys can never collide with symbol keys.
        PropertyKey(fxhash::hash64(s.as_bytes()) & !SYMBOL_KEY_FLAG)
    }

    /// Property key for a symbol with the given registry id. The high bit marks
    /// the key as a symbol; the id is carried directly (no hashing, no collisions).
    pub fn from_symbol(id: u32) -> Self {
        PropertyKey(SYMBOL_KEY_FLAG | (id as u64))
    }

    /// True if this key names a symbol property (excluded from for-in/Object.keys).
    pub fn is_symbol(&self) -> bool {
        self.0 & SYMBOL_KEY_FLAG != 0
    }

    pub fn symbol_id(&self) -> Option<u32> {
        if self.is_symbol() {
            Some((self.0 & !SYMBOL_KEY_FLAG) as u32)
        } else {
            None
        }
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

/// High bit of a PropertyKey marks symbol keys (see PropertyKey::from_symbol).
const SYMBOL_KEY_FLAG: u64 = 1 << 63;

impl std::fmt::Debug for Shape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shape")
            .field("id", &self.id)
            .field("entry_count", &self.property_count)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> (PropertyKey, usize) {
        (PropertyKey::from_string(name), 0)
    }

    #[test]
    fn test_intern_defaults_attrs() {
        let s = Shape::intern(
            vec![key("f5_def_a"), (PropertyKey::from_string("f5_def_b"), 1)],
            vec!["f5_def_a".to_string(), "f5_def_b".to_string()],
        );
        assert_eq!(s.attrs, vec![ATTR_DEFAULT, ATTR_DEFAULT]);
        assert_eq!(s.attr_at(0), ATTR_DEFAULT);
        assert_eq!(s.attr_at(1), ATTR_DEFAULT);
        // Same entries → same interned shape (hash-consed).
        let s2 = Shape::intern(
            vec![key("f5_def_a"), (PropertyKey::from_string("f5_def_b"), 1)],
            vec!["f5_def_a".to_string(), "f5_def_b".to_string()],
        );
        assert!(std::ptr::eq(s, s2));
    }

    #[test]
    fn test_attrs_fork_identity() {
        // Same keys/offsets, different attributes → different shapes/ids,
        // so IC (shape.id → slot) caches stay sound when A4 writes attrs.
        let entries = || vec![(PropertyKey::from_string("f5_fork"), 0)];
        let names = || vec!["f5_fork".to_string()];
        let def = Shape::intern(entries(), names());
        let ro = Shape::intern_with_attrs(
            entries(),
            names(),
            vec![ATTR_ENUMERABLE | ATTR_CONFIGURABLE],
        );
        assert!(!std::ptr::eq(def, ro));
        assert_ne!(def.id, ro.id);
        assert_eq!(ro.attr_at(0), ATTR_ENUMERABLE | ATTR_CONFIGURABLE);
        assert_eq!(def.attr_at(0), ATTR_DEFAULT);
        // Re-interning the same attrs hits the same shape.
        let ro2 = Shape::intern_with_attrs(
            entries(),
            names(),
            vec![ATTR_ENUMERABLE | ATTR_CONFIGURABLE],
        );
        assert!(std::ptr::eq(ro, ro2));
    }

    #[test]
    fn test_parent_extension_defaults() {
        let parent = Shape::intern(vec![key("f5_par")], vec!["f5_par".to_string()]);
        let child = Shape::intern_with_parent(
            parent,
            PropertyKey::from_string("f5_kid"),
            "f5_kid".to_string(),
        );
        assert_eq!(child.attrs, vec![ATTR_DEFAULT, ATTR_DEFAULT]);
        assert_eq!(child.attr_at(5), ATTR_DEFAULT, "out-of-range reads default");
    }

    #[test]
    fn test_attr_flag_bits() {
        assert_eq!(
            ATTR_DEFAULT,
            ATTR_WRITABLE | ATTR_ENUMERABLE | ATTR_CONFIGURABLE
        );
        assert_eq!(ATTR_WRITABLE, 0b001);
        assert_eq!(ATTR_ENUMERABLE, 0b010);
        assert_eq!(ATTR_CONFIGURABLE, 0b100);
    }
}
