use anyhow::Result;

use crate::cli::ConfigAction;
use crate::config::store::{self, read_config, write_config};

pub fn run(action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Set { key, value } => {
            let mut config = read_config()?;
            config.insert(key.clone(), serde_json::Value::String(value.clone()));
            write_config(&config)?;
            println!("\x1b[32m✓\x1b[0m Set \"{key}\" to \"{value}\"");
            println!(
                "\x1b[2m  Config file: {}\x1b[0m",
                store::config_path()?.display()
            );
        }
        ConfigAction::Get { key } => {
            let config = read_config()?;
            match config.get(&key) {
                Some(v) => match v.as_str() {
                    Some(s) => println!("{s}"),
                    None => println!("{v}"),
                },
                None => {
                    println!("\x1b[33mNot set\x1b[0m");
                }
            }
        }
        ConfigAction::List => {
            let config = read_config()?;
            if config.is_empty() {
                println!("\x1b[33mNo configuration settings found.\x1b[0m");
                println!(
                    "\x1b[2mConfig file: {}\x1b[0m",
                    store::config_path()?.display()
                );
            } else {
                println!("{}", serde_json::to_string_pretty(&config)?);
                println!(
                    "\x1b[2m\nConfig file: {}\x1b[0m",
                    store::config_path()?.display()
                );
            }
        }
        ConfigAction::Path => {
            println!("{}", store::config_path()?.display());
        }
    }
    Ok(())
}
