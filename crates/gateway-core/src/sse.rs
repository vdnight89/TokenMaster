//! 通用 SSE 事件流解析器（增量喂入，容忍 chunk 边界与 CRLF）。
//! 供各 Provider 消费上游 `text/event-stream`（zcode/minimax/trae 等）。

use std::collections::VecDeque;

#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
    data_lines: Vec<String>,
    pending: VecDeque<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一段原始字节；内部按行切分（\n，兼容 \r\n），空行即一个事件块。
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let mut line = &line[..line.len() - 1]; // 去 \n
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            self.process_line(&String::from_utf8_lossy(line));
        }
    }

    /// 上游结束时冲刷半截事件。
    pub fn finalize(&mut self) {
        if !self.buf.is_empty() {
            let rest = String::from_utf8_lossy(&self.buf).to_string();
            self.buf.clear();
            for line in rest.split('\n') {
                self.process_line(line.trim_end_matches('\r'));
            }
        }
        if !self.data_lines.is_empty() {
            let joined = std::mem::take(&mut self.data_lines).join("\n");
            self.pending.push_back(joined);
        }
    }

    /// 取下一条完整的 data 载荷（多行 data 按 SSE 规范以 \n 连接）。
    pub fn next_data(&mut self) -> Option<String> {
        self.pending.pop_front()
    }

    fn process_line(&mut self, line: &str) {
        if line.is_empty() {
            if !self.data_lines.is_empty() {
                let joined = std::mem::take(&mut self.data_lines).join("\n");
                self.pending.push_back(joined);
            }
            return;
        }
        if let Some(d) = line.strip_prefix("data:") {
            // SSE 规范：去掉一个前导空格
            self.data_lines.push(d.strip_prefix(' ').unwrap_or(d).to_string());
        }
        // `event:`/`:` 注释行不参与载荷
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_split_across_chunks() {
        let mut p = SseParser::new();
        p.feed(b"event: message_start\ndata: {\"a\":1}\n");
        p.feed(b"\ndata: {\"b\":2}\n\n");
        assert_eq!(p.next_data().unwrap(), "{\"a\":1}");
        assert_eq!(p.next_data().unwrap(), "{\"b\":2}");
        assert!(p.next_data().is_none());
    }

    #[test]
    fn crlf_and_multiline_data() {
        let mut p = SseParser::new();
        p.feed(b"data: line1\r\ndata: line2\r\n\r\n");
        assert_eq!(p.next_data().unwrap(), "line1\nline2");
    }

    #[test]
    fn finalize_flushes_trailing_event_without_blank_line() {
        let mut p = SseParser::new();
        p.feed(b"data: tail");
        p.finalize();
        assert_eq!(p.next_data().unwrap(), "tail");
    }
}
