use std::env;
use std::time::Duration;

use anyhow::Result;

use crate::anki::client::{DEFAULT_URL, anki_client};
use crate::cli::DoctorArgs;
use crate::config::store::{config_path, read_config};
use crate::doctor::render::{print_check, print_header, print_section_title};
use crate::doctor::report::{CheckResult, mask};
use crate::llm::provider::resolve_model;
use crate::spinner::llm_spinner;
use crate::workspace::context::Workspace;

const PROBE_TIMEOUT_SECS: u64 = 8;

pub fn run(args: DoctorArgs) -> Result<()> {
    let mut had_failure = false;

    print_header(env!("CARGO_PKG_VERSION"));

    had_failure |= print_environment_section();
    had_failure |= print_workspace_section();
    had_failure |= print_llm_section(args.check);
    had_failure |= print_tts_section();
    had_failure |= print_anki_section();

    println!();
    if had_failure {
        eprintln!("Some checks failed.");
        std::process::exit(1);
    }
    Ok(())
}

fn emit(check: CheckResult, had_failure: &mut bool) {
    if matches!(check.status, crate::doctor::report::Status::Fail) {
        *had_failure = true;
    }
    print_check(&check);
}

// ---------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------

fn print_environment_section() -> bool {
    print_section_title("Environment");
    let mut failed = false;

    match config_path() {
        Ok(p) => {
            let detail = if p.exists() {
                p.display().to_string()
            } else {
                format!("{} (not created)", p.display())
            };
            emit(CheckResult::ok("Config path", detail), &mut failed);
        }
        Err(e) => emit(CheckResult::fail("Config path", e.to_string()), &mut failed),
    }

    match read_config() {
        Ok(_) => emit(CheckResult::ok("Config parse", "valid"), &mut failed),
        Err(e) => emit(
            CheckResult::fail("Config parse", e.to_string()),
            &mut failed,
        ),
    }

    failed
}

// ---------------------------------------------------------------------------
// Workspace
// ---------------------------------------------------------------------------

fn print_workspace_section() -> bool {
    print_section_title("Workspace");
    let mut failed = false;

    match Workspace::effective() {
        Some(ws) => {
            emit(
                CheckResult::ok("Workspace", ws.root.display().to_string()),
                &mut failed,
            );
            match ws.manifest.default_model.as_deref() {
                Some(m) if !m.is_empty() => {
                    emit(CheckResult::ok("default_model", m.to_string()), &mut failed);
                }
                _ => emit(
                    CheckResult::skip("default_model", "unset".to_string()),
                    &mut failed,
                ),
            }
        }
        None => emit(
            CheckResult::skip("Workspace", "none detected".to_string()),
            &mut failed,
        ),
    }

    let resolved = resolve_model(None);
    let known = crate::llm::pricing::KNOWN_MODELS.contains(&resolved.as_str());
    let check = if known {
        CheckResult::ok("Resolved model", resolved)
    } else {
        CheckResult::warn(
            "Resolved model",
            format!("{resolved} (not in known-models list, typo or custom?)"),
        )
    };
    emit(check, &mut failed);

    failed
}

// ---------------------------------------------------------------------------
// LLM providers
// ---------------------------------------------------------------------------

struct LlmProvider {
    name: &'static str,
    env_var: &'static str,
    base_url: &'static str,
    /// Cheapest known model for this provider, used for `--check` probes.
    probe_model: &'static str,
}

const LLM_PROVIDERS: &[LlmProvider] = &[
    LlmProvider {
        name: "OpenAI",
        env_var: "OPENAI_API_KEY",
        base_url: "https://api.openai.com/v1",
        probe_model: "gpt-4.1-nano",
    },
    LlmProvider {
        name: "Gemini",
        env_var: "GEMINI_API_KEY",
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        probe_model: "gemini-2.5-flash-lite",
    },
    LlmProvider {
        name: "DeepSeek",
        env_var: "DEEPSEEK_API_KEY",
        base_url: "https://api.deepseek.com",
        probe_model: "deepseek-v4-flash",
    },
];

fn print_llm_section(check: bool) -> bool {
    print_section_title("LLM Providers");
    let mut failed = false;

    for p in LLM_PROVIDERS {
        match env_value(p.env_var) {
            Some(key) => {
                let label = format!("{} ({})", p.name, p.env_var);
                let masked = mask(&key);
                if check {
                    let pb = llm_spinner(format!(
                        "probing {} via {} (1-token completion)",
                        p.name, p.probe_model
                    ));
                    let result = probe_chat(p.base_url, &key, p.probe_model);
                    pb.finish_and_clear();
                    match result {
                        Ok(()) => emit(
                            CheckResult::ok(
                                label,
                                format!(
                                    "POST {}/chat/completions ({}) → 200 ({masked})",
                                    p.base_url, p.probe_model
                                ),
                            ),
                            &mut failed,
                        ),
                        Err(msg) => emit(CheckResult::fail(label, msg), &mut failed),
                    }
                } else {
                    emit(
                        CheckResult::ok(label, format!("set ({masked})")),
                        &mut failed,
                    );
                }
            }
            None => emit(
                CheckResult::skip(format!("{} ({})", p.name, p.env_var), "unset".to_string()),
                &mut failed,
            ),
        }
    }

    let global_key = env_value("ANKI_LLM_API_KEY");
    let cfg_base = read_config().ok().and_then(|c| c.api_base_url);
    match (global_key.as_deref(), cfg_base.as_deref()) {
        (Some(k), Some(base)) => {
            let masked = mask(k);
            let label = format!("Custom base URL: {base}");
            if check {
                let pb = llm_spinner(format!("probing {base}/models"));
                let result = probe_models(base, k);
                pb.finish_and_clear();
                match result {
                    Ok(status) => emit(
                        CheckResult::ok(label, format!("GET {base}/models → {status} ({masked})")),
                        &mut failed,
                    ),
                    Err(msg) => emit(CheckResult::fail(label, msg), &mut failed),
                }
            } else {
                emit(
                    CheckResult::ok(label, format!("ANKI_LLM_API_KEY set ({masked})")),
                    &mut failed,
                );
            }
        }
        (Some(k), None) => {
            emit(
                CheckResult::warn(
                    "ANKI_LLM_API_KEY",
                    format!("set ({}) but api_base_url not configured", mask(k)),
                ),
                &mut failed,
            );
        }
        (None, Some(base)) => {
            emit(
                CheckResult::warn(
                    "Custom base URL",
                    format!("{base} configured but ANKI_LLM_API_KEY unset"),
                ),
                &mut failed,
            );
        }
        (None, None) => {}
    }

    failed
}

fn probe_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(PROBE_TIMEOUT_SECS)))
        .build()
        .into()
}

/// Send a 1-token chat completion to verify auth, model access, and balance.
fn probe_chat(base_url: &str, api_key: &str, model: &str) -> Result<(), String> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 1,
    });
    match probe_agent()
        .post(&url)
        .header("Authorization", &format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .send_json(&body)
    {
        Ok(_) => Ok(()),
        Err(ureq::Error::StatusCode(code)) => Err(http_error_msg(code, model)),
        Err(e) => Err(format!("connection failed: {e}")),
    }
}

/// `GET /models` fallback used for custom base URLs where we don't know
/// which model the endpoint serves (OpenRouter, Ollama, etc.).
fn probe_models(base_url: &str, api_key: &str) -> Result<u16, String> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    match probe_agent()
        .get(&url)
        .header("Authorization", &format!("Bearer {api_key}"))
        .call()
    {
        Ok(resp) => Ok(resp.status().as_u16()),
        Err(ureq::Error::StatusCode(code)) => match code {
            401 | 403 => Err(format!("HTTP {code}: invalid API key")),
            404 => Err(format!("HTTP {code}: /models not available at {base_url}")),
            other => Err(format!("HTTP {other}")),
        },
        Err(e) => Err(format!("connection failed: {e}")),
    }
}

fn http_error_msg(code: u16, model: &str) -> String {
    match code {
        401 | 403 => format!("HTTP {code}: invalid API key"),
        402 => format!("HTTP {code}: payment required (insufficient balance?)"),
        404 => format!("HTTP {code}: model {model} not found"),
        429 => format!("HTTP {code}: rate-limited or quota exhausted"),
        other => format!("HTTP {other}"),
    }
}

// ---------------------------------------------------------------------------
// TTS providers (offline reporting only)
// ---------------------------------------------------------------------------

fn print_tts_section() -> bool {
    print_section_title("TTS Providers");
    let mut failed = false;
    let cfg = read_config().ok();

    let openai = env_value("OPENAI_API_KEY").or_else(|| env_value("ANKI_LLM_API_KEY"));
    match openai {
        Some(k) => emit(
            CheckResult::ok("OpenAI TTS", format!("key ({})", mask(&k))),
            &mut failed,
        ),
        None => emit(
            CheckResult::skip("OpenAI TTS", "no key".to_string()),
            &mut failed,
        ),
    }

    let azure_key =
        env_value("AZURE_TTS_KEY").or_else(|| cfg.as_ref().and_then(|c| c.azure_tts_key.clone()));
    let azure_region = env_value("AZURE_TTS_REGION")
        .or_else(|| cfg.as_ref().and_then(|c| c.azure_tts_region.clone()));
    let azure_check = match (azure_key.as_deref(), azure_region.as_deref()) {
        (Some(k), Some(r)) => {
            CheckResult::ok("Azure TTS", format!("key ({}), region {r}", mask(k)))
        }
        (Some(_), None) => CheckResult::warn("Azure TTS", "key set, region missing".to_string()),
        (None, Some(r)) => CheckResult::warn("Azure TTS", format!("region {r}, key missing")),
        (None, None) => CheckResult::skip("Azure TTS", "no credentials".to_string()),
    };
    emit(azure_check, &mut failed);

    let google_key =
        env_value("GOOGLE_TTS_KEY").or_else(|| cfg.as_ref().and_then(|c| c.google_tts_key.clone()));
    match google_key {
        Some(k) => emit(
            CheckResult::ok("Google TTS", format!("key ({})", mask(&k))),
            &mut failed,
        ),
        None => emit(
            CheckResult::skip("Google TTS", "no key".to_string()),
            &mut failed,
        ),
    }

    let aws_id = env_value("AWS_ACCESS_KEY_ID")
        .or_else(|| cfg.as_ref().and_then(|c| c.aws_tts_access_key_id.clone()));
    let aws_secret = env_value("AWS_SECRET_ACCESS_KEY").or_else(|| {
        cfg.as_ref()
            .and_then(|c| c.aws_tts_secret_access_key.clone())
    });
    let aws_region = env_value("AWS_REGION")
        .or_else(|| env_value("AWS_DEFAULT_REGION"))
        .or_else(|| cfg.as_ref().and_then(|c| c.aws_tts_region.clone()));
    let polly_check = match (
        aws_id.as_deref(),
        aws_secret.as_deref(),
        aws_region.as_deref(),
    ) {
        (Some(id), Some(_), Some(r)) => {
            CheckResult::ok("Amazon Polly", format!("key ({}), region {r}", mask(id)))
        }
        (Some(_), Some(_), None) => CheckResult::warn(
            "Amazon Polly",
            "credentials set, region missing".to_string(),
        ),
        (None, None, None) => CheckResult::skip("Amazon Polly", "no credentials".to_string()),
        _ => CheckResult::warn("Amazon Polly", "incomplete credentials".to_string()),
    };
    emit(polly_check, &mut failed);

    failed
}

// ---------------------------------------------------------------------------
// AnkiConnect
// ---------------------------------------------------------------------------

fn print_anki_section() -> bool {
    print_section_title("AnkiConnect");
    let mut failed = false;

    let url = read_config()
        .ok()
        .and_then(|c| c.anki_connect_url)
        .unwrap_or_else(|| DEFAULT_URL.to_string());
    emit(CheckResult::ok("URL", url.clone()), &mut failed);

    let pb = llm_spinner(format!("pinging AnkiConnect at {url}"));
    let result = anki_client().request_no_params::<u32>("version");
    pb.finish_and_clear();
    match result {
        Ok(v) => emit(
            CheckResult::ok("Connection", format!("AnkiConnect v{v}")),
            &mut failed,
        ),
        Err(e) => emit(CheckResult::fail("Connection", e.to_string()), &mut failed),
    }

    failed
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn env_value(var: &str) -> Option<String> {
    env::var(var).ok().filter(|v| !v.trim().is_empty())
}
