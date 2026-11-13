use crate::providers::error::ProviderError;

#[derive(Default)]
pub(crate) struct SseParser {
    buffer: Vec<u8>,
    /// Byte offset the next boundary scan resumes from, so repeated pushes
    /// of partial events do not rescan the whole buffer each time.
    scan_offset: usize,
}

impl SseParser {
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, ProviderError> {
        if self.buffer.len().saturating_add(chunk.len()) > 4 * 1024 * 1024 {
            return Err(ProviderError::Stream("SSE buffer exceeded 4 MiB".into()));
        }
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();

        while let Some((event_end, separator_len)) =
            find_event_boundary(&self.buffer, self.scan_offset)
        {
            let event_bytes = self.buffer[..event_end].to_vec();
            self.buffer.drain(..event_end + separator_len);
            self.scan_offset = 0;
            if let Some(event) = parse_sse_event(&event_bytes)? {
                events.push(event);
            }
        }
        // A separator can straddle chunk boundaries; keep the last 3 bytes
        // in scan range for the next push.
        self.scan_offset = self.buffer.len().saturating_sub(3);

        Ok(events)
    }

    pub(crate) fn finish(&mut self) -> Result<Option<String>, ProviderError> {
        if self.buffer.is_empty() {
            return Ok(None);
        }
        Err(ProviderError::Stream("Incomplete SSE event at EOF".into()))
    }
}

fn find_event_boundary(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    let mut index = start.min(bytes.len());
    while index < bytes.len() {
        if bytes[index..].starts_with(b"\r\n\r\n") {
            return Some((index, 4));
        }
        if bytes[index..].starts_with(b"\n\n") {
            return Some((index, 2));
        }
        index += 1;
    }
    None
}

fn parse_sse_event(bytes: &[u8]) -> Result<Option<String>, ProviderError> {
    let text = std::str::from_utf8(bytes)?;
    let mut data_lines = Vec::new();

    for raw_line in text.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            let data = rest.strip_prefix(' ').unwrap_or(rest);
            data_lines.push(data);
        }
    }

    if data_lines.is_empty() {
        return Ok(None);
    }

    Ok(Some(data_lines.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::SseParser;

    #[test]
    fn sse_parser_reassembles_split_events() {
        let mut parser = SseParser::default();
        let first = parser
            .push(
                b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,",
            )
            .unwrap();
        assert!(first.is_empty());

        let second = parser
            .push(b"\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n")
            .unwrap();

        assert_eq!(
            second,
            vec![
                "{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}"
                    .to_string()
            ]
        );
    }

    #[test]
    fn sse_parser_extracts_multiple_events_from_one_chunk() {
        let mut parser = SseParser::default();
        let events = parser
            .push(b"data: one\n\ndata: two\r\n\r\ndata: thr")
            .unwrap();

        assert_eq!(events, vec!["one".to_string(), "two".to_string()]);

        let tail = parser.push(b"ee\n\n").unwrap();
        assert_eq!(tail, vec!["three".to_string()]);
    }
}
