//! Collect functions from two programs.

mod function;
mod generics;
mod path;
mod symbol;

pub use function::FunctionCollector;
pub use generics::TypeCollector;
pub use symbol::TraitCollector;
