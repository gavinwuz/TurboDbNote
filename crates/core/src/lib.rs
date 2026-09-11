//! UI-independent document and query state. Runtime results never enter `.note` files.
pub mod notebook;
pub mod query;

pub use notebook::{Cell, CellContent, Note, NoteError, TaskStatus};
pub use query::{QueryState, QueryStatus, QueryToken};
