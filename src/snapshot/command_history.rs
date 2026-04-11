use anyhow::Result;

use crate::style::style;

use super::store::list_snapshots;

pub fn run() -> Result<()> {
    let snapshots = list_snapshots()?;

    if snapshots.is_empty() {
        eprintln!("No snapshots found.");
        return Ok(());
    }

    let s = style();
    eprintln!(
        "{:<22} {:<24} {:<18} {:>5}  {}",
        s.bold("Run ID"),
        s.bold("Deck"),
        s.bold("Model"),
        s.bold("Notes"),
        s.bold("Status"),
    );
    eprintln!("{}", s.dim("─".repeat(78)));

    for snap in &snapshots {
        let status = if snap.rolled_back {
            s.muted("rolled back").to_string()
        } else {
            "ok".to_string()
        };
        eprintln!(
            "{:<22} {:<24} {:<18} {:>5}  {}",
            snap.run_id, snap.deck, snap.model, snap.note_count, status
        );
    }

    Ok(())
}
