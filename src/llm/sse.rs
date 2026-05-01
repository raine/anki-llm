#[derive(Debug, Clone, Default, PartialEq)]
pub struct SseEvent {
    pub data: String,
}

const MAX_BUFFER_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
    cur_data: String,
    have_field: bool,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &[u8]) -> anyhow::Result<Vec<SseEvent>> {
        self.buf.extend_from_slice(chunk);
        if self.buf.len() > MAX_BUFFER_BYTES {
            anyhow::bail!("SSE frame exceeded {MAX_BUFFER_BYTES} bytes without a delimiter");
        }

        let mut out = Vec::new();
        while let Some((content_end, delim_len)) = find_double_newline(&self.buf) {
            let frame: Vec<u8> = self.buf.drain(..content_end).collect();
            self.buf.drain(..delim_len);
            let text = String::from_utf8_lossy(&frame);
            for raw in text.split('\n') {
                let line = raw.strip_suffix('\r').unwrap_or(raw);
                if line.is_empty() || line.starts_with(':') {
                    continue;
                }
                let (field, value) = match line.split_once(':') {
                    Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                    None => (line, ""),
                };
                self.have_field = true;
                if field == "data" {
                    if !self.cur_data.is_empty() {
                        self.cur_data.push('\n');
                    }
                    self.cur_data.push_str(value);
                }
            }
            if self.have_field && !self.cur_data.is_empty() {
                out.push(SseEvent {
                    data: std::mem::take(&mut self.cur_data),
                });
            }
            self.have_field = false;
            self.cur_data.clear();
        }
        Ok(out)
    }

    pub fn flush(mut self) -> Option<SseEvent> {
        if !self.buf.is_empty() {
            let text = String::from_utf8_lossy(&self.buf);
            for raw in text.split('\n') {
                let line = raw.strip_suffix('\r').unwrap_or(raw);
                if line.is_empty() || line.starts_with(':') {
                    continue;
                }
                let (field, value) = match line.split_once(':') {
                    Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                    None => (line, ""),
                };
                self.have_field = true;
                if field == "data" {
                    if !self.cur_data.is_empty() {
                        self.cur_data.push('\n');
                    }
                    self.cur_data.push_str(value);
                }
            }
            self.buf.clear();
        }
        if !self.have_field || self.cur_data.is_empty() {
            return None;
        }
        Some(SseEvent {
            data: std::mem::take(&mut self.cur_data),
        })
    }
}

fn find_double_newline(buf: &[u8]) -> Option<(usize, usize)> {
    let mut best = find_subslice(buf, b"\n\n").map(|i| (i, 2));
    if let Some(i) = find_subslice(buf, b"\r\n\r\n") {
        match best {
            Some((j, _)) if j < i => {}
            _ => best = Some((i, 4)),
        }
    }
    best
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_split_lf_frame() {
        let mut parser = SseParser::new();
        assert!(parser.feed(b"data: he").unwrap().is_empty());
        assert!(parser.feed(b"llo\n").unwrap().is_empty());
        let events = parser.feed(b"\n").unwrap();
        assert_eq!(
            events,
            vec![SseEvent {
                data: "hello".into()
            }]
        );
    }

    #[test]
    fn parses_crlf_frame() {
        let mut parser = SseParser::new();
        let events = parser.feed(b"data: hi\r\n\r\n").unwrap();
        assert_eq!(events, vec![SseEvent { data: "hi".into() }]);
    }

    #[test]
    fn ignores_comments_and_joins_data_lines() {
        let mut parser = SseParser::new();
        let events = parser.feed(b": ping\n\ndata: one\ndata: two\n\n").unwrap();
        assert_eq!(
            events,
            vec![SseEvent {
                data: "one\ntwo".into()
            }]
        );
    }

    #[test]
    fn flush_emits_trailing_event() {
        let mut parser = SseParser::new();
        assert!(parser.feed(b"data: tail").unwrap().is_empty());
        assert_eq!(
            parser.flush(),
            Some(SseEvent {
                data: "tail".into()
            })
        );
    }
}
