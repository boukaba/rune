pub mod value;
pub mod string;
pub mod shape;
pub mod object;
pub mod function;
pub mod gc;
pub mod heap;
pub mod barrier;

pub mod prelude {
    pub use crate::value::Value;
    pub use crate::string::HeapString;
    pub use crate::shape::Shape;
    pub use crate::object::JSObject;
    pub use crate::function::Func;
    pub use crate::gc::SemiSpace;
}
