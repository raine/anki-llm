use std::io::{self, BufRead};

/// Copy prompt to clipboard and read response from stdin.
/// Returns the pasted response text.
pub fn get_llm_response_manually(prompt: &str) -> Result<String, anyhow::Error> {
    // Try to copy to clipboard
    match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(prompt.to_string())) {
        Ok(()) => eprintln!("Prompt copied to clipboard."),
        Err(_) => {
            eprintln!("\nCould not copy to clipboard. Copy the prompt manually:");
            eprintln!("{}", "-".repeat(60));
            eprintln!("{prompt}");
            eprintln!("{}", "-".repeat(60));
        }
    }

    eprintln!("\nPlease follow these steps:");
    eprintln!("  1. Paste the prompt into your preferred LLM.");
    eprintln!("  2. Copy the full JSON response from the LLM.");
    eprintln!("  3. Paste the response here in the terminal.");
    eprintln!("  4. Type \"END\" on a new line and press Enter.");
    eprintln!("\nWaiting for LLM response...");

    let stdin = io::stdin();
    let mut lines = Vec::new();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim() == "END" {
            break;
        }
        lines.push(line);
    }

    let response = lines.join("\n").trim().to_string();
    if response.is_empty() {
        anyhow::bail!("No response received from stdin");
    }

    eprintln!("Response received. Processing...\n");
    Ok(response)
}
