//! UI-independent document and query state. Runtime results never enter `.note` files.
pub mod mongo;
pub mod notebook;
pub mod odbc;
pub mod query;

pub use notebook::{Cell, CellContent, Note, NoteError, TaskItem, TaskStatus};
pub use query::{QueryState, QueryStatus, QueryToken};

pub mod timeouts;
