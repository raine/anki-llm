use anyhow::Result;

use crate::style::style;

use super::store::list_snapshots;

fn truncate_display(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cutoff = max.saturating_sub(1);
        let mut out: String = s.chars().take(cutoff).collect();
        out.push('…');
        out
    }
}

pub fn run() -> Result<()> {
    let snapshots = list_snapshots()?;

    if snapshots.is_empty() {
        eprintln!("No snapshots found.");
        return Ok(());
    }

    let s = style();
    eprintln!(
        "{} {} {} {}  {}",
        s.bold(format!("{:<22}", "Run ID")),
        s.bold(format!("{:<32}", "Source")),
        s.bold(format!("{:<18}", "Model")),
        s.bold(format!("{:>5}", "Notes")),
        s.bold("Status"),
    );
    eprintln!("{}", s.dim("─".repeat(86)));

    for snap in &snapshots {
        let status = if snap.rolled_back {
            s.muted("rolled back").to_string()
        } else {
            "ok".to_string()
        };
        let source = truncate_display(&snap.source_display(), 32);
        eprintln!(
            "{:<22} {:<32} {:<18} {:>5}  {}",
            snap.run_id, source, snap.model, snap.note_count, status
        );
    }

    Ok(())
}
