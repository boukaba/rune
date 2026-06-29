// TODO: document safety invariants for each unsafe fn (Sprint 14+)
#![allow(clippy::missing_safety_doc)]

pub mod array;
pub mod barrier;
pub mod env;
pub mod float;
pub mod function;
pub mod gc;
pub mod heap;
pub mod object;
pub mod promise;
pub mod shape;
pub mod string_object;
pub mod string;
pub mod value;

pub mod prelude {
    pub use crate::float::HeapFloat64;
    pub use crate::function::Func;
    pub use crate::gc::SemiSpace;
    pub use crate::object::JSObject;
    pub use crate::shape::Shape;
    pub use crate::string::HeapString;
    pub use crate::value::Value;
}
