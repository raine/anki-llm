use crate::doctor::report::{CheckResult, Status};
use crate::style::style;

pub fn print_header(version: &str) {
    let s = style();
    println!(
        "{} {}",
        s.bold("anki-llm doctor"),
        s.muted(format!("v{version}"))
    );
}

pub fn print_section_title(title: &str) {
    println!();
    println!("{}", style().accent(title));
}

pub fn print_check(check: &CheckResult) {
    let s = style();
    let symbol = match check.status {
        Status::Ok => s.success("✓"),
        Status::Warn => s.warning("⚠"),
        Status::Fail => s.error_text("✗"),
        Status::Skip => s.muted("·"),
    };
    let label = match check.status {
        Status::Skip => s.muted(&check.label),
        _ => check.label.clone(),
    };
    match &check.detail {
        Some(d) => println!("  {symbol} {label}  {}", s.muted(d)),
        None => println!("  {symbol} {label}"),
    }
}
