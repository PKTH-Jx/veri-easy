//! Collect functions from two programs.

mod function;
mod path;
mod symbol;
mod types;

pub use function::FunctionCollector;
pub use path::PathResolver;
pub use symbol::SymbolCollector;
pub use types::TypeCollector;
