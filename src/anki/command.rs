use anyhow::{Result, bail};

use crate::anki::client::anki_client;
use crate::cli::QueryArgs;

const ANKI_CONNECT_DOCS: &str = include_str!("../../ANKI_CONNECT.md");

pub fn run(args: QueryArgs) -> Result<()> {
    if args.action == "docs" || args.action == "help" {
        print!("{ANKI_CONNECT_DOCS}");
        return Ok(());
    }

    let params: Option<serde_json::Value> = match &args.params {
        Some(json_str) => {
            let val: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
                anyhow::anyhow!(
                    "invalid JSON in params argument: {e}\n\n\
                     Params must be valid JSON. Example: '{{\"query\":\"deck:Default\"}}'"
                )
            })?;
            Some(val)
        }
        None => None,
    };

    let client = anki_client();
    match client.request_raw(&args.action, params) {
        Ok(result) => {
            println!("{}", serde_json::to_string_pretty(&result)?);
            Ok(())
        }
        Err(e) => {
            bail!(
                "{e}\n\n\
                 Make sure:\n  \
                 1. Anki Desktop is running\n  \
                 2. AnkiConnect add-on is installed (code: 2055492159)\n  \
                 3. The action '{}' is valid\n  \
                 4. The params are correctly formatted for this action\n\n\
                 See ANKI_CONNECT.md for documentation on available actions and their parameters.",
                args.action
            );
        }
    }
}
