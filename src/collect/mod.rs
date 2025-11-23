//! Collect functions from two programs.

mod function;
mod path;
mod symbol;
mod types;
mod precond;

pub use function::FunctionCollector;
pub use path::PathResolver;
pub use symbol::SymbolCollector;
pub use types::TypeCollector;
pub use precond::collect_preconds;
