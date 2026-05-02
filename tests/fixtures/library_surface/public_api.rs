use clap::Parser;

fn main() {
    let cli = anki_llm::cli::Cli::parse_from(["anki-llm", "docs"]);
    let _run_cli: fn(anki_llm::cli::Cli) -> anyhow::Result<()> = anki_llm::run_cli;
    let _client = anki_llm::anki::client::AnkiClient::new;
    let _params: Option<anki_llm::anki::schema::AddNoteParams> = None;
    drop(cli);
}
