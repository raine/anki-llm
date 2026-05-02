#[derive(Debug, PartialEq)]
pub(super) enum Segment {
    Thinking(String),
    Answer(String),
}

pub(super) struct TagSplitter {
    open_tag: &'static str,
    close_tag: &'static str,
    in_thinking: bool,
    buffer: String,
}

impl TagSplitter {
    pub(super) fn new(open_tag: &'static str, close_tag: &'static str) -> Self {
        Self {
            open_tag,
            close_tag,
            in_thinking: false,
            buffer: String::new(),
        }
    }

    pub(super) fn push(&mut self, chunk: &str) -> Vec<Segment> {
        self.buffer.push_str(chunk);
        let mut out = Vec::new();
        loop {
            let target = if self.in_thinking {
                self.close_tag
            } else {
                self.open_tag
            };
            if let Some(idx) = self.buffer.find(target) {
                if !self.in_thinking && idx > 0 && !self.buffer[..idx].trim().is_empty() {
                    let emit_len = idx + target.len();
                    let segment: String = self.buffer.drain(..emit_len).collect();
                    out.push(Segment::Answer(segment));
                    continue;
                }
                if idx > 0 {
                    let segment: String = self.buffer.drain(..idx).collect();
                    out.push(self.classify(segment));
                }
                self.buffer.drain(..target.len());
                self.in_thinking = !self.in_thinking;
                if !self.in_thinking && self.buffer.starts_with('\n') {
                    self.buffer.drain(..1);
                }
            } else {
                let hold = partial_suffix_len(&self.buffer, target);
                let emit_len = self.buffer.len() - hold;
                if emit_len > 0 {
                    let segment: String = self.buffer.drain(..emit_len).collect();
                    out.push(self.classify(segment));
                }
                break;
            }
        }
        out
    }

    pub(super) fn flush(mut self) -> Option<Segment> {
        if self.buffer.is_empty() {
            None
        } else {
            let text = std::mem::take(&mut self.buffer);
            Some(self.classify(text))
        }
    }

    fn classify(&self, text: String) -> Segment {
        if self.in_thinking {
            Segment::Thinking(text)
        } else {
            Segment::Answer(text)
        }
    }
}

fn partial_suffix_len(buf: &str, tag: &str) -> usize {
    let max = std::cmp::min(tag.len() - 1, buf.len());
    for i in (1..=max).rev() {
        if buf.ends_with(&tag[..i]) {
            return i;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_split_tag_until_complete() {
        let mut splitter = TagSplitter::new("<thought>", "</thought>");

        assert_eq!(splitter.push("<tho"), Vec::new());
        assert_eq!(
            splitter.push("ught>plan"),
            vec![Segment::Thinking("plan".into())]
        );
        assert_eq!(splitter.push("</thou"), Vec::new());
        assert_eq!(
            splitter.push("ght>[{}]"),
            vec![Segment::Answer("[{}]".into())]
        );
        assert_eq!(splitter.flush(), None);
    }

    #[test]
    fn flushes_pending_thinking_segment() {
        let mut splitter = TagSplitter::new("<thought>", "</thought>");

        assert_eq!(
            splitter.push("<thought>plan"),
            vec![Segment::Thinking("plan".into())]
        );
        assert_eq!(splitter.flush(), None);
    }
}
