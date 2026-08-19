use std::cell::RefCell;

/// Well-known symbol ids (registered at thread_local init, ids are stable).
pub const SYM_ITERATOR: u32 = 0;
pub const SYM_MATCH: u32 = 1;
pub const SYM_REPLACE: u32 = 2;
pub const SYM_SEARCH: u32 = 3;
pub const SYM_SPLIT: u32 = 4;
pub const SYM_TO_PRIMITIVE: u32 = 5;
pub const SYM_HAS_INSTANCE: u32 = 6;
pub const SYM_TO_STRING_TAG: u32 = 7;
pub const SYM_SPECIES: u32 = 8;
pub const SYM_IS_CONCAT_SPREADABLE: u32 = 9;
pub const SYM_UNSCOPABLES: u32 = 10;
pub const SYM_MATCH_ALL: u32 = 11;
pub const SYM_ASYNC_ITERATOR: u32 = 12;

/// Symbol registry: descriptions (by symbol id) and the global Symbol.for
/// key registry. Thread-local, never touched by the GC (symbol Values carry
/// only the registry id as their inline payload).
struct SymbolRegistry {
    descriptions: Vec<Option<String>>,
    for_registry: Vec<(String, u32)>,
}

thread_local! {
    static SYMBOL_REGISTRY: RefCell<SymbolRegistry> = RefCell::new(SymbolRegistry {
        descriptions: vec![
            Some("Symbol.iterator".to_string()),
            Some("Symbol.match".to_string()),
            Some("Symbol.replace".to_string()),
            Some("Symbol.search".to_string()),
            Some("Symbol.split".to_string()),
            Some("Symbol.toPrimitive".to_string()),
            Some("Symbol.hasInstance".to_string()),
            Some("Symbol.toStringTag".to_string()),
            Some("Symbol.species".to_string()),
            Some("Symbol.isConcatSpreadable".to_string()),
            Some("Symbol.unscopables".to_string()),
            Some("Symbol.matchAll".to_string()),
            Some("Symbol.asyncIterator".to_string()),
        ],
        for_registry: Vec::new(),
    });
}

/// Register a new unique symbol with the given description (None = no description).
pub fn register_symbol(description: Option<String>) -> u32 {
    SYMBOL_REGISTRY.with(|r| {
        let mut reg = r.borrow_mut();
        reg.descriptions.push(description);
        (reg.descriptions.len() - 1) as u32
    })
}

/// The description of a symbol (None if it has none).
pub fn symbol_description(id: u32) -> Option<String> {
    SYMBOL_REGISTRY.with(|r| {
        let reg = r.borrow();
        reg.descriptions.get(id as usize).and_then(|d| d.clone())
    })
}

/// Display form: `Symbol(desc)` or `Symbol()` for description-less symbols.
pub fn symbol_display(id: u32) -> String {
    match symbol_description(id) {
        Some(d) => format!("Symbol({d})"),
        None => "Symbol()".to_string(),
    }
}

/// Symbol.for: return the registered symbol for `key`, creating it if needed.
pub fn symbol_for(key: &str) -> u32 {
    SYMBOL_REGISTRY.with(|r| {
        let mut reg = r.borrow_mut();
        if let Some((_, id)) = reg.for_registry.iter().find(|(k, _)| k == key) {
            return *id;
        }
        reg.descriptions.push(Some(key.to_string()));
        let id = (reg.descriptions.len() - 1) as u32;
        reg.for_registry.push((key.to_string(), id));
        id
    })
}

/// Symbol.keyFor: return the registry key for a symbol, if it was registered via Symbol.for.
pub fn symbol_key_for(id: u32) -> Option<String> {
    SYMBOL_REGISTRY.with(|r| {
        let reg = r.borrow();
        reg.for_registry
            .iter()
            .find(|(_, i)| *i == id)
            .map(|(k, _)| k.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_well_known_ids() {
        assert_eq!(
            symbol_description(SYM_ITERATOR).as_deref(),
            Some("Symbol.iterator")
        );
        assert_eq!(symbol_display(SYM_MATCH), "Symbol(Symbol.match)");
    }

    #[test]
    fn test_register_unique() {
        let a = register_symbol(Some("desc".to_string()));
        let b = register_symbol(Some("desc".to_string()));
        assert_ne!(a, b);
        assert_eq!(symbol_display(a), "Symbol(desc)");
    }

    #[test]
    fn test_for_registry() {
        let a = symbol_for("foo");
        let b = symbol_for("foo");
        assert_eq!(a, b);
        let c = symbol_for("bar");
        assert_ne!(a, c);
        assert_eq!(symbol_key_for(a).as_deref(), Some("foo"));
        let unique = register_symbol(None);
        assert_eq!(symbol_key_for(unique), None);
    }
}
