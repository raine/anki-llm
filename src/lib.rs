mod app;

pub mod anki;
pub(crate) mod audio;
pub(crate) mod batch;
pub mod cli;
pub(crate) mod config;
pub(crate) mod data;
pub(crate) mod docs;
pub(crate) mod doctor;
pub(crate) mod generate;
pub(crate) mod llm;
pub(crate) mod note_type;
pub(crate) mod snapshot;
pub(crate) mod spinner;
pub(crate) mod style;
pub(crate) mod template;
pub(crate) mod tts;
pub(crate) mod tui;
pub(crate) mod update;
pub(crate) mod workspace;

pub fn run_cli(cli: cli::Cli) -> anyhow::Result<()> {
    app::run(cli)
}
