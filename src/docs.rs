use anyhow::Result;

const DOCUMENTATION: &str = include_str!("../docs/generated/llms-full.txt");

pub fn run() -> Result<()> {
    print!("{DOCUMENTATION}");
    Ok(())
}
