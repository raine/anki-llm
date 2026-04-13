//! Soft credential probe for the voices TUI.
//!
//! This is deliberately separate from `tts::runtime` / `tts::spec` —
//! those eager resolvers bail hard when credentials are missing, which
//! is correct for batch synthesis but wrong for a browser that must
//! keep working even if the user only has one provider's keys set.

use std::collections::HashMap;

use crate::config::store::{AppConfig, read_config};
use crate::tts::provider::{ProviderSelection, amazon, azure, google};

use super::catalog::ProviderId;

#[derive(Debug, Clone)]
pub enum ProviderPreviewState {
    Ready {
        selection: ProviderSelection,
        endpoint_identity: Option<String>,
    },
    Unavailable {
        reason: String,
    },
}

impl ProviderPreviewState {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }
}

/// Probe every provider and return a per-provider readiness state.
/// Never fails.
pub fn probe_all() -> HashMap<ProviderId, ProviderPreviewState> {
    let config = read_config().ok();
    let mut out = HashMap::new();
    out.insert(ProviderId::Openai, probe_openai(config.as_ref()));
    out.insert(ProviderId::Azure, probe_azure(config.as_ref()));
    out.insert(ProviderId::Google, probe_google(config.as_ref()));
    out.insert(ProviderId::Amazon, probe_amazon(config.as_ref()));
    out
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn probe_openai(config: Option<&AppConfig>) -> ProviderPreviewState {
    // ANKI_LLM_API_KEY > OPENAI_API_KEY — matches spec.rs precedence.
    let api_key = env_nonempty("ANKI_LLM_API_KEY").or_else(|| env_nonempty("OPENAI_API_KEY"));
    let api_base_url = config.and_then(|c| c.api_base_url.clone());

    if api_key.is_none() && api_base_url.is_none() {
        return ProviderPreviewState::Unavailable {
            reason: "set OPENAI_API_KEY (or ANKI_LLM_API_KEY)".into(),
        };
    }

    ProviderPreviewState::Ready {
        selection: ProviderSelection::OpenAi {
            api_key,
            api_base_url: api_base_url.clone(),
        },
        endpoint_identity: api_base_url,
    }
}

fn probe_azure(config: Option<&AppConfig>) -> ProviderPreviewState {
    let subscription_key =
        env_nonempty("AZURE_TTS_KEY").or_else(|| config.and_then(|c| c.azure_tts_key.clone()));
    let Some(subscription_key) = subscription_key else {
        return ProviderPreviewState::Unavailable {
            reason: "set AZURE_TTS_KEY (or azure_tts_key in config)".into(),
        };
    };
    let region = env_nonempty("AZURE_TTS_REGION")
        .or_else(|| config.and_then(|c| c.azure_tts_region.clone()));
    let Some(region) = region else {
        return ProviderPreviewState::Unavailable {
            reason: "set AZURE_TTS_REGION (or azure_tts_region in config)".into(),
        };
    };

    let endpoint = Some(azure::endpoint_identity(&region));
    ProviderPreviewState::Ready {
        selection: ProviderSelection::Azure {
            subscription_key,
            region,
        },
        endpoint_identity: endpoint,
    }
}

fn probe_google(config: Option<&AppConfig>) -> ProviderPreviewState {
    let api_key =
        env_nonempty("GOOGLE_TTS_KEY").or_else(|| config.and_then(|c| c.google_tts_key.clone()));
    let Some(api_key) = api_key else {
        return ProviderPreviewState::Unavailable {
            reason: "set GOOGLE_TTS_KEY (or google_tts_key in config)".into(),
        };
    };
    ProviderPreviewState::Ready {
        selection: ProviderSelection::Google { api_key },
        endpoint_identity: Some(google::endpoint_identity()),
    }
}

fn probe_amazon(config: Option<&AppConfig>) -> ProviderPreviewState {
    let access_key_id = env_nonempty("AWS_ACCESS_KEY_ID")
        .or_else(|| config.and_then(|c| c.aws_tts_access_key_id.clone()));
    let Some(access_key_id) = access_key_id else {
        return ProviderPreviewState::Unavailable {
            reason: "set AWS_ACCESS_KEY_ID (or aws_tts_access_key_id in config)".into(),
        };
    };
    let secret_access_key = env_nonempty("AWS_SECRET_ACCESS_KEY")
        .or_else(|| config.and_then(|c| c.aws_tts_secret_access_key.clone()));
    let Some(secret_access_key) = secret_access_key else {
        return ProviderPreviewState::Unavailable {
            reason: "set AWS_SECRET_ACCESS_KEY (or aws_tts_secret_access_key in config)".into(),
        };
    };
    let region = env_nonempty("AWS_REGION")
        .or_else(|| env_nonempty("AWS_DEFAULT_REGION"))
        .or_else(|| config.and_then(|c| c.aws_tts_region.clone()));
    let Some(region) = region else {
        return ProviderPreviewState::Unavailable {
            reason: "set AWS_REGION (or aws_tts_region in config)".into(),
        };
    };
    let session_token = env_nonempty("AWS_SESSION_TOKEN");

    let endpoint = Some(amazon::endpoint_identity(&region));
    ProviderPreviewState::Ready {
        selection: ProviderSelection::Amazon {
            access_key_id,
            secret_access_key,
            region,
            session_token,
        },
        endpoint_identity: endpoint,
    }
}
