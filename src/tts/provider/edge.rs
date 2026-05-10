use std::io::{Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use http::header::HeaderName;
use rand::Rng;
use sha2::{Digest, Sha256};
use std::sync::Once;
use tungstenite::client::IntoClientRequest;
use tungstenite::{Message, WebSocket, connect};

use super::{AudioFormat, RenderProfile, SynthesisRequest, TtsProvider};
use crate::tts::error::TtsError;

const TRUSTED_CLIENT_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";
const SYNTH_ENDPOINT_IDENTITY: &str =
    "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1";
const SYNTH_URL: &str = "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1?TrustedClientToken=";
const OUTPUT_FORMAT: &str = "audio-24khz-48kbitrate-mono-mp3";
const CHROMIUM_FULL_VERSION: &str = "143.0.3650.75";
const SEC_MS_GEC_VERSION: &str = "1-143.0.3650.75";
const WIN_EPOCH_SECONDS: u64 = 11_644_473_600;

pub fn endpoint_identity() -> String {
    SYNTH_ENDPOINT_IDENTITY.to_string()
}

pub struct EdgeTtsProvider;

impl EdgeTtsProvider {
    pub fn new() -> Self {
        Self
    }
}

impl TtsProvider for EdgeTtsProvider {
    fn id(&self) -> &'static str {
        "edge"
    }

    fn render_profile(&self) -> RenderProfile {
        RenderProfile::EdgeSsml
    }

    fn synthesize(&self, req: &SynthesisRequest) -> Result<Vec<u8>, TtsError> {
        output_format(req.format)?;
        install_rustls_provider();
        let request = connect_request(&random_request_id())?;
        let (mut socket, _) = connect(request).map_err(map_transport_error)?;
        let request_id = random_request_id();
        synthesize_on_socket(&mut socket, &request_id, &req.payload)
    }
}

fn install_rustls_provider() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn output_format(fmt: AudioFormat) -> Result<&'static str, TtsError> {
    match fmt.ext() {
        "mp3" => Ok(OUTPUT_FORMAT),
        other => Err(TtsError::Permanent(format!(
            "edge provider does not support format '{other}'"
        ))),
    }
}

fn random_request_id() -> String {
    let mut buf = [0u8; 16];
    rand::rng().fill_bytes(&mut buf);
    hex(&buf)
}

fn connect_request(muid: &str) -> Result<tungstenite::handshake::client::Request, TtsError> {
    let url = format!(
        "{SYNTH_URL}{TRUSTED_CLIENT_TOKEN}&ConnectionId={connection_id}&Sec-MS-GEC={gec}&Sec-MS-GEC-Version={SEC_MS_GEC_VERSION}",
        connection_id = random_request_id(),
        gec = generate_sec_ms_gec(now_unix_secs()? as u64),
    );
    let mut request = url
        .into_client_request()
        .map_err(|e| TtsError::Permanent(format!("invalid edge websocket request: {e}")))?;
    for (name, value) in edge_headers(muid) {
        request.headers_mut().insert(
            HeaderName::from_static(name),
            value
                .parse()
                .map_err(|e| TtsError::Permanent(format!("invalid edge header value: {e}")))?,
        );
    }
    Ok(request)
}

fn edge_headers(muid: &str) -> [(&'static str, String); 8] {
    let major = CHROMIUM_FULL_VERSION
        .split_once('.')
        .map(|(major, _)| major)
        .unwrap_or(CHROMIUM_FULL_VERSION);
    [
        ("pragma", "no-cache".to_string()),
        ("cache-control", "no-cache".to_string()),
        (
            "origin",
            "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold".to_string(),
        ),
        ("sec-websocket-version", "13".to_string()),
        (
            "user-agent",
            format!(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36 \
                 Edg/{major}.0.0.0"
            ),
        ),
        ("accept-encoding", "gzip, deflate, br, zstd".to_string()),
        ("accept-language", "en-US,en;q=0.9".to_string()),
        ("cookie", format!("muid={};", muid.to_ascii_uppercase())),
    ]
}

fn now_unix_secs() -> Result<u64, TtsError> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| TtsError::Permanent(format!("system clock before unix epoch: {e}")))?
        .as_secs())
}

fn generate_sec_ms_gec(unix_secs: u64) -> String {
    let rounded = unix_secs - (unix_secs % 300);
    let windows_ticks = (rounded + WIN_EPOCH_SECONDS) * 10_000_000;
    let input = format!("{windows_ticks}{TRUSTED_CLIENT_TOKEN}");
    let digest = Sha256::digest(input.as_bytes());
    hex_upper(&digest)
}

fn synthesize_on_socket<S: Read + Write>(
    socket: &mut WebSocket<S>,
    request_id: &str,
    ssml: &str,
) -> Result<Vec<u8>, TtsError> {
    socket
        .send(Message::Text(speech_config_message().into()))
        .map_err(map_transport_error)?;
    socket
        .send(Message::Text(ssml_message(request_id, ssml).into()))
        .map_err(map_transport_error)?;

    let mut audio = Vec::new();
    loop {
        let msg = socket.read().map_err(map_transport_error)?;
        match msg {
            Message::Text(text) => {
                let headers = parse_message_headers(&text)?;
                if has_header(&headers, "Path", "turn.end") {
                    if has_header(&headers, "X-RequestId", request_id) {
                        if audio.is_empty() {
                            return Err(TtsError::Transient(
                                "empty audio response from edge".to_string(),
                            ));
                        }
                        return Ok(audio);
                    }
                    return Err(TtsError::Transient(
                        "edge turn.end had mismatched request id".to_string(),
                    ));
                }
            }
            Message::Binary(bytes) => {
                if let Some(body) = audio_body_for_request(&bytes, request_id)? {
                    audio.extend_from_slice(body);
                }
            }
            Message::Close(_) => {
                return Err(TtsError::Transient(
                    "edge websocket closed before turn.end".to_string(),
                ));
            }
            _ => {}
        }
    }
}

fn speech_config_message() -> String {
    format!(
        "Content-Type:application/json; charset=utf-8\r\nPath:speech.config\r\n\r\n{{\"context\":{{\"synthesis\":{{\"audio\":{{\"metadataoptions\":{{\"sentenceBoundaryEnabled\":false,\"wordBoundaryEnabled\":true}},\"outputFormat\":\"{OUTPUT_FORMAT}\"}}}}}}}}"
    )
}

fn ssml_message(request_id: &str, ssml: &str) -> String {
    format!(
        "X-RequestId:{request_id}\r\nContent-Type:application/ssml+xml\r\nPath:ssml\r\n\r\n{ssml}"
    )
}

fn audio_body_for_request<'a>(
    bytes: &'a [u8],
    request_id: &str,
) -> Result<Option<&'a [u8]>, TtsError> {
    if bytes.len() < 2 {
        return Err(TtsError::Transient(
            "edge binary frame missing header length".to_string(),
        ));
    }
    let header_len = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
    let body_start = header_len + 2;
    if bytes.len() < body_start {
        return Err(TtsError::Transient(format!(
            "edge binary frame too short: {} bytes for {header_len}-byte header",
            bytes.len()
        )));
    }
    let header_text = std::str::from_utf8(&bytes[2..body_start])
        .map_err(|e| TtsError::Transient(format!("edge binary headers were not utf-8: {e}")))?;
    let headers = parse_headers(header_text);
    if !has_header(&headers, "Path", "audio") {
        return Ok(None);
    }
    if !has_header(&headers, "X-RequestId", request_id) {
        return Err(TtsError::Transient(
            "edge audio frame had mismatched request id".to_string(),
        ));
    }
    Ok(Some(&bytes[body_start..]))
}

fn parse_message_headers(text: &str) -> Result<Vec<(String, String)>, TtsError> {
    let Some((headers, _)) = text.split_once("\r\n\r\n") else {
        return Err(TtsError::Transient(
            "edge text frame missing header separator".to_string(),
        ));
    };
    Ok(parse_headers(headers))
}

fn parse_headers(s: &str) -> Vec<(String, String)> {
    s.split("\r\n")
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

fn has_header(headers: &[(String, String)], key: &str, value: &str) -> bool {
    headers.iter().any(|(k, v)| k == key && v == value)
}

fn map_transport_error(e: tungstenite::Error) -> TtsError {
    TtsError::Transient(e.to_string())
}

fn hex(bytes: &[u8]) -> String {
    encode_hex(bytes, b"0123456789abcdef")
}

fn hex_upper(bytes: &[u8]) -> String {
    encode_hex(bytes, b"0123456789ABCDEF")
}

fn encode_hex(bytes: &[u8], alphabet: &[u8; 16]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(alphabet[(b >> 4) as usize] as char);
        out.push(alphabet[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_profile_and_endpoint_identity() {
        let p = EdgeTtsProvider::new();
        assert_eq!(p.id(), "edge");
        assert_eq!(p.render_profile(), RenderProfile::EdgeSsml);
        assert_eq!(
            endpoint_identity(),
            "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1"
        );
        assert!(format!("{SYNTH_URL}{TRUSTED_CLIENT_TOKEN}").contains(TRUSTED_CLIENT_TOKEN));
    }

    #[test]
    fn sec_ms_gec_matches_current_edge_formula() {
        assert_eq!(
            generate_sec_ms_gec(1_764_844_875),
            "FA745D283B047F2CC96C41EAB94CC6696E2F42C3E06218329C20B349E9197A9B"
        );
    }

    #[test]
    fn connect_request_contains_current_edge_auth_query_and_headers() {
        let req = connect_request("0123456789abcdef0123456789abcdef").unwrap();
        let uri = req.uri().to_string();
        assert!(uri.contains("TrustedClientToken=6A5AA1D4EAFF4E9FB37E23D68491D6F4"));
        assert!(uri.contains("ConnectionId="));
        assert!(uri.contains("Sec-MS-GEC="));
        assert!(uri.contains("Sec-MS-GEC-Version=1-143.0.3650.75"));
        assert_eq!(
            req.headers().get("origin").unwrap(),
            "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold"
        );
        assert!(
            req.headers()
                .get("user-agent")
                .unwrap()
                .to_str()
                .unwrap()
                .contains("Edg/143")
        );
        assert_eq!(
            req.headers().get("cookie").unwrap(),
            "muid=0123456789ABCDEF0123456789ABCDEF;"
        );
    }

    #[test]
    fn request_messages_match_edge_contract() {
        let speech = speech_config_message();
        assert!(speech.starts_with("Content-Type:application/json; charset=utf-8\r\n"));
        assert!(speech.contains("Path:speech.config\r\n\r\n"));
        assert!(speech.contains("\"outputFormat\":\"audio-24khz-48kbitrate-mono-mp3\""));

        let ssml = ssml_message("abc123", "<speak />");
        assert!(ssml.starts_with("X-RequestId:abc123\r\n"));
        assert!(ssml.contains("Content-Type:application/ssml+xml\r\n"));
        assert!(ssml.contains("Path:ssml\r\n\r\n<speak />"));
    }

    #[test]
    fn binary_audio_body_extracts_matching_audio() {
        let frame = binary_frame("X-RequestId:req1\r\nPath:audio", b"abc");
        assert_eq!(
            audio_body_for_request(&frame, "req1").unwrap(),
            Some(&b"abc"[..])
        );
    }

    #[test]
    fn binary_audio_body_ignores_non_audio_frames() {
        let frame = binary_frame("X-RequestId:req1\r\nPath:metadata", b"abc");
        assert!(audio_body_for_request(&frame, "req1").unwrap().is_none());
    }

    #[test]
    fn binary_audio_body_rejects_mismatched_request_id() {
        let frame = binary_frame("X-RequestId:req2\r\nPath:audio", b"abc");
        assert!(matches!(
            audio_body_for_request(&frame, "req1"),
            Err(TtsError::Transient(_))
        ));
    }

    #[test]
    fn binary_audio_body_rejects_short_frames() {
        assert!(matches!(
            audio_body_for_request(&[0, 10, b'a'], "req1"),
            Err(TtsError::Transient(_))
        ));
    }

    #[test]
    fn text_headers_require_separator() {
        assert!(matches!(
            parse_message_headers("Path:turn.end"),
            Err(TtsError::Transient(_))
        ));
        let headers = parse_message_headers("Path:turn.end\r\nX-RequestId:req\r\n\r\n").unwrap();
        assert!(has_header(&headers, "Path", "turn.end"));
        assert!(has_header(&headers, "X-RequestId", "req"));
    }

    fn binary_frame(headers: &str, body: &[u8]) -> Vec<u8> {
        let len = headers.len() as u16;
        let mut frame = Vec::new();
        frame.extend_from_slice(&len.to_be_bytes());
        frame.extend_from_slice(headers.as_bytes());
        frame.extend_from_slice(body);
        frame
    }
}
