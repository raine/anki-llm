pub mod command_process_deck;
pub mod command_process_file;
pub mod controller;
pub mod deck_mode;
pub mod engine;
pub mod error;
// Fields are consumed by renderers in the binary crate; lib crate warns falsely.
#[allow(dead_code)]
pub mod events;
pub mod file_mode;
pub mod plain;
pub mod preview;
pub mod process_row;
pub mod report;
pub mod session;
pub mod tui;
