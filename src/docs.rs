use anyhow::Result;

const README: &str = include_str!("../README.md");

pub fn run() -> Result<()> {
    print!("{README}");
    Ok(())
}
