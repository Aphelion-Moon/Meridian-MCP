// Decode only complete lines so UTF-8 characters split across pipe reads survive.
// An oversized line makes analysis incomplete but does not stop later evidence.
pub(crate) const MAX_LINE_BYTES: usize = 1024 * 1024;

#[derive(Default)]
pub(crate) struct BoundedLines {
    line: Vec<u8>,
    oversized: bool,
    pub oversized_lines: u64,
}

impl BoundedLines {
    pub fn push(&mut self, bytes: &[u8], mut observe: impl FnMut(&str)) {
        for part in bytes.split_inclusive(|byte| *byte == b'\n') {
            let newline = part.last() == Some(&b'\n');
            let part = if newline {
                &part[..part.len() - 1]
            } else {
                part
            };
            if !self.oversized {
                if self.line.len() + part.len() > MAX_LINE_BYTES {
                    self.oversized = true;
                    self.line.clear();
                } else {
                    self.line.extend_from_slice(part);
                }
            }
            if newline {
                self.finish(&mut observe);
            }
        }
    }

    pub fn finish(&mut self, mut observe: impl FnMut(&str)) {
        if self.oversized {
            self.oversized_lines += 1;
        } else {
            observe(&String::from_utf8_lossy(&self.line));
        }
        self.line.clear();
        self.oversized = false;
    }
}
