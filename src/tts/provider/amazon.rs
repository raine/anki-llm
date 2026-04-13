use std::io::Read;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{AudioFormat, SynthesisRequest, TextFormat, TtsProvider};
use crate::tts::error::TtsError;

const TIMEOUT_SECS: u64 = 120;
const SERVICE: &str = "polly";
const DEFAULT_ENGINE: &str = "standard";

fn host_for(region: &str) -> String {
    format!("polly.{region}.amazonaws.com")
}

fn endpoint_for(region: &str) -> String {
    format!("https://{}/v1/speech", host_for(region))
}

/// Stable cache endpoint identity. Pins the cache to a specific region
/// so moving between e.g. `us-east-1` and `eu-west-1` doesn't serve audio
/// across voices that happen to share a name.
pub fn endpoint_identity(region: &str) -> String {
    format!("https://polly.{region}.amazonaws.com")
}

pub struct AmazonTtsProvider {
    region: String,
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
    agent: ureq::Agent,
}

impl AmazonTtsProvider {
    pub fn new(
        access_key_id: String,
        secret_access_key: String,
        region: String,
        session_token: Option<String>,
    ) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(TIMEOUT_SECS)))
            .build()
            .into();
        Self {
            region,
            access_key_id,
            secret_access_key,
            session_token,
            agent,
        }
    }
}

#[derive(Serialize)]
struct SynthesizeRequest<'a> {
    #[serde(rename = "OutputFormat")]
    output_format: &'a str,
    #[serde(rename = "Text")]
    text: &'a str,
    #[serde(rename = "TextType")]
    text_type: &'a str,
    #[serde(rename = "VoiceId")]
    voice_id: &'a str,
    #[serde(rename = "Engine")]
    engine: &'a str,
}

fn output_format(fmt: AudioFormat) -> Result<&'static str, TtsError> {
    match fmt.ext() {
        "mp3" => Ok("mp3"),
        other => Err(TtsError::Permanent(format!(
            "amazon provider does not support format '{other}'"
        ))),
    }
}

impl TtsProvider for AmazonTtsProvider {
    fn id(&self) -> &'static str {
        "amazon"
    }

    fn text_format(&self) -> TextFormat {
        TextFormat::PlainText
    }

    fn synthesize(&self, req: &SynthesisRequest) -> Result<Vec<u8>, TtsError> {
        let body = SynthesizeRequest {
            output_format: output_format(req.format)?,
            text: &req.payload,
            text_type: "text",
            voice_id: &req.voice,
            engine: req.model.as_deref().unwrap_or(DEFAULT_ENGINE),
        };
        let body_bytes = serde_json::to_vec(&body)
            .map_err(|e| TtsError::Permanent(format!("failed to serialize polly request: {e}")))?;

        let url = endpoint_for(&self.region);
        let host = host_for(&self.region);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| TtsError::Permanent(format!("system clock before unix epoch: {e}")))?
            .as_secs();
        let (amz_date, date_stamp) = format_amz_dates(now);

        let auth = sign_v4(SigV4Inputs {
            method: "POST",
            host: &host,
            path: "/v1/speech",
            query: "",
            body: &body_bytes,
            region: &self.region,
            service: SERVICE,
            access_key_id: &self.access_key_id,
            secret_access_key: &self.secret_access_key,
            session_token: self.session_token.as_deref(),
            amz_date: &amz_date,
            date_stamp: &date_stamp,
        });

        let mut request = self
            .agent
            .post(&url)
            .header("Host", &host)
            .header("Content-Type", "application/json")
            .header("X-Amz-Date", &amz_date)
            .header("Authorization", &auth)
            .header("User-Agent", "anki-llm");
        if let Some(ref token) = self.session_token {
            request = request.header("X-Amz-Security-Token", token);
        }

        let mut response = request.send(&body_bytes).map_err(map_ureq_error)?;

        let mut buf = Vec::new();
        response
            .body_mut()
            .as_reader()
            .read_to_end(&mut buf)
            .map_err(|e| TtsError::Transient(format!("body read failed: {e}")))?;
        if buf.is_empty() {
            return Err(TtsError::Transient(
                "empty audio response from amazon polly".to_string(),
            ));
        }
        Ok(buf)
    }
}

// --------------------------------------------------------------------------
// SigV4 signing
// --------------------------------------------------------------------------

struct SigV4Inputs<'a> {
    method: &'a str,
    host: &'a str,
    path: &'a str,
    query: &'a str,
    body: &'a [u8],
    region: &'a str,
    service: &'a str,
    access_key_id: &'a str,
    secret_access_key: &'a str,
    session_token: Option<&'a str>,
    amz_date: &'a str,
    date_stamp: &'a str,
}

/// Sign an AWS request with SigV4 and return the full `Authorization`
/// header value. This is the minimal subset of SigV4 needed for Polly:
/// a single POST to a fixed path with no query string, JSON body, and
/// an optional `X-Amz-Security-Token` header for temporary credentials.
fn sign_v4(inp: SigV4Inputs<'_>) -> String {
    let payload_hash = hex_sha256(inp.body);

    let mut signed_headers_list: Vec<&str> = vec!["content-type", "host", "x-amz-date"];
    if inp.session_token.is_some() {
        signed_headers_list.push("x-amz-security-token");
    }
    signed_headers_list.sort_unstable();
    let signed_headers = signed_headers_list.join(";");

    let mut canonical_headers = String::new();
    for name in &signed_headers_list {
        let value = match *name {
            "content-type" => "application/json",
            "host" => inp.host,
            "x-amz-date" => inp.amz_date,
            "x-amz-security-token" => inp.session_token.unwrap_or(""),
            _ => "",
        };
        canonical_headers.push_str(name);
        canonical_headers.push(':');
        canonical_headers.push_str(value.trim());
        canonical_headers.push('\n');
    }

    let canonical_request = format!(
        "{method}\n{path}\n{query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}",
        method = inp.method,
        path = inp.path,
        query = inp.query,
    );

    let credential_scope = format!(
        "{}/{}/{}/aws4_request",
        inp.date_stamp, inp.region, inp.service
    );
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{hash}",
        amz_date = inp.amz_date,
        scope = credential_scope,
        hash = hex_sha256(canonical_request.as_bytes()),
    );

    let k_secret = format!("AWS4{}", inp.secret_access_key);
    let k_date = hmac_sha256(k_secret.as_bytes(), inp.date_stamp.as_bytes());
    let k_region = hmac_sha256(&k_date, inp.region.as_bytes());
    let k_service = hmac_sha256(&k_region, inp.service.as_bytes());
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    let signature = hex(&hmac_sha256(&k_signing, string_to_sign.as_bytes()));

    format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/{scope}, SignedHeaders={signed}, Signature={sig}",
        access_key = inp.access_key_id,
        scope = credential_scope,
        signed = signed_headers,
        sig = signature,
    )
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex(&h.finalize())
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// HMAC-SHA256 implemented on top of `sha2::Sha256`. Keeps the dep graph
/// small — we don't pull in `hmac` just for this one call site.
fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let digest = {
            let mut h = Sha256::new();
            h.update(key);
            h.finalize()
        };
        k[..digest.len()].copy_from_slice(&digest);
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }

    let inner = {
        let mut h = Sha256::new();
        h.update(ipad);
        h.update(msg);
        h.finalize()
    };
    let outer = {
        let mut h = Sha256::new();
        h.update(opad);
        h.update(inner);
        h.finalize()
    };
    outer.to_vec()
}

/// Format (amz_date, date_stamp) strings for SigV4 from a unix timestamp.
/// Returns e.g. ("20260413T123456Z", "20260413") — computed manually so
/// we don't take a new time-crate dependency just for formatting.
fn format_amz_dates(epoch_secs: u64) -> (String, String) {
    let secs_of_day = epoch_secs % 86_400;
    let days = (epoch_secs / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    let h = secs_of_day / 3600;
    let min = (secs_of_day / 60) % 60;
    let s = secs_of_day % 60;
    let date_stamp = format!("{y:04}{m:02}{d:02}");
    let amz_date = format!("{y:04}{m:02}{d:02}T{h:02}{min:02}{s:02}Z");
    (amz_date, date_stamp)
}

/// Convert days-from-unix-epoch to (year, month, day). Implementation of
/// Howard Hinnant's `civil_from_days` algorithm, valid for all dates from
/// -32768-01-01 to 32767-12-31.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

// --------------------------------------------------------------------------
// Error mapping
// --------------------------------------------------------------------------

/// Map Polly HTTP status codes to the transient/permanent split.
///
/// - 400 InvalidSampleRate / LexiconNotFound / TextLengthExceeded → permanent.
/// - 403 Forbidden / InvalidSignature → permanent (creds or clock skew).
/// - 404 VoiceNotFound → permanent.
/// - 429 too many requests → transient.
/// - 500/503 service failure → transient.
fn map_ureq_error(e: ureq::Error) -> TtsError {
    match e {
        ureq::Error::StatusCode(429) => TtsError::Transient("HTTP 429: rate limited".to_string()),
        ureq::Error::StatusCode(code) if code >= 500 => {
            TtsError::Transient(format!("HTTP {code}: server error"))
        }
        ureq::Error::StatusCode(code) => {
            TtsError::Permanent(format!("HTTP {code}: non-retryable error"))
        }
        other => TtsError::Transient(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_and_endpoint_formatting() {
        assert_eq!(host_for("us-east-1"), "polly.us-east-1.amazonaws.com");
        assert_eq!(
            endpoint_for("eu-west-1"),
            "https://polly.eu-west-1.amazonaws.com/v1/speech"
        );
        assert_eq!(
            endpoint_identity("us-west-2"),
            "https://polly.us-west-2.amazonaws.com"
        );
    }

    #[test]
    fn id_and_text_format() {
        let p = AmazonTtsProvider::new(
            "AKIDEXAMPLE".into(),
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            "us-east-1".into(),
            None,
        );
        assert_eq!(p.id(), "amazon");
        assert_eq!(p.text_format(), TextFormat::PlainText);
    }

    #[test]
    fn error_mapping_status_codes() {
        assert!(matches!(
            map_ureq_error(ureq::Error::StatusCode(429)),
            TtsError::Transient(_)
        ));
        assert!(matches!(
            map_ureq_error(ureq::Error::StatusCode(500)),
            TtsError::Transient(_)
        ));
        assert!(matches!(
            map_ureq_error(ureq::Error::StatusCode(403)),
            TtsError::Permanent(_)
        ));
        assert!(matches!(
            map_ureq_error(ureq::Error::StatusCode(400)),
            TtsError::Permanent(_)
        ));
    }

    // ---- Date math ------------------------------------------------------

    #[test]
    fn format_amz_dates_known_values() {
        // 2015-08-30 12:36:00 UTC: the canonical SigV4 test-suite timestamp.
        let epoch = 1_440_938_160;
        let (amz, stamp) = format_amz_dates(epoch);
        assert_eq!(stamp, "20150830");
        assert_eq!(amz, "20150830T123600Z");
    }

    #[test]
    fn format_amz_dates_leap_day() {
        // 2024-02-29 00:00:00 UTC.
        let epoch = 1_709_164_800;
        let (amz, stamp) = format_amz_dates(epoch);
        assert_eq!(stamp, "20240229");
        assert_eq!(amz, "20240229T000000Z");
    }

    #[test]
    fn format_amz_dates_epoch() {
        let (amz, stamp) = format_amz_dates(0);
        assert_eq!(stamp, "19700101");
        assert_eq!(amz, "19700101T000000Z");
    }

    // ---- HMAC / signature ----------------------------------------------

    #[test]
    fn signing_key_derivation_matches_aws_reference() {
        // From AWS's "Example: Signed request using Authorization header"
        // worked example: the derived signing key for
        // 20150830 / us-east-1 / iam with the canonical test secret.
        // https://docs.aws.amazon.com/IAM/latest/UserGuide/signing-elements.html
        let k_secret = format!("AWS4{}", "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY");
        let k_date = hmac_sha256(k_secret.as_bytes(), b"20150830");
        let k_region = hmac_sha256(&k_date, b"us-east-1");
        let k_service = hmac_sha256(&k_region, b"iam");
        let k_signing = hmac_sha256(&k_service, b"aws4_request");
        assert_eq!(
            hex(&k_signing),
            "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
        );
    }

    #[test]
    fn hmac_sha256_known_vector() {
        // RFC 4231 test case 1.
        let key = [0x0bu8; 20];
        let msg = b"Hi There";
        let mac = hmac_sha256(&key, msg);
        assert_eq!(
            hex(&mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn sigv4_matches_aws_reference() {
        // AWS's public SigV4 test suite: `get-vanilla` without query,
        // adapted for a minimal POST to confirm our canonicalization.
        // We don't have an exact upstream fixture for Polly, so this is
        // a smoke test that signing is deterministic given fixed inputs.
        let sig1 = sign_v4(SigV4Inputs {
            method: "POST",
            host: "polly.us-east-1.amazonaws.com",
            path: "/v1/speech",
            query: "",
            body: b"{}",
            region: "us-east-1",
            service: "polly",
            access_key_id: "AKIDEXAMPLE",
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            session_token: None,
            amz_date: "20150830T123600Z",
            date_stamp: "20150830",
        });
        let sig2 = sign_v4(SigV4Inputs {
            method: "POST",
            host: "polly.us-east-1.amazonaws.com",
            path: "/v1/speech",
            query: "",
            body: b"{}",
            region: "us-east-1",
            service: "polly",
            access_key_id: "AKIDEXAMPLE",
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            session_token: None,
            amz_date: "20150830T123600Z",
            date_stamp: "20150830",
        });
        assert_eq!(sig1, sig2, "signing must be deterministic");
        assert!(sig1.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/polly/aws4_request, SignedHeaders=content-type;host;x-amz-date, Signature="));
    }

    #[test]
    fn sigv4_with_session_token_signs_that_header() {
        let sig = sign_v4(SigV4Inputs {
            method: "POST",
            host: "polly.us-east-1.amazonaws.com",
            path: "/v1/speech",
            query: "",
            body: b"{}",
            region: "us-east-1",
            service: "polly",
            access_key_id: "AKIDEXAMPLE",
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            session_token: Some("FwoGZXIvYXdzEXAMPLE"),
            amz_date: "20150830T123600Z",
            date_stamp: "20150830",
        });
        assert!(sig.contains("SignedHeaders=content-type;host;x-amz-date;x-amz-security-token"));
    }
}
