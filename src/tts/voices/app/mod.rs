//! Ratatui event loop for the voice browser.
//!
//! Layout: left pane is a filtered voice list with a visible facet chip
//! row and text search; right pane shows the highlighted voice's details
//! and the YAML scaffold that is copied to the clipboard on Enter.

mod draw;
mod runner;
mod state;
mod update;

pub use runner::run;
pub use state::InitialFilters;
