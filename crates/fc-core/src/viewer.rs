//! Internal viewer documents (SPEC §5.1): text, hex, image and folder summaries.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// How the viewer presents a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewerMode {
    Text,
    Hex,
    Image,
    Folder,
}

/// Files above this size are truncated in the viewer.
pub const VIEW_LIMIT: u64 = 32 * 1024 * 1024;

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "ico", "svg"];

/// Picks the initial mode: folders, images by extension, NUL bytes → hex, else text.
pub fn detect_mode(path: &Path) -> ViewerMode {
    if path.is_dir() {
        return ViewerMode::Folder;
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return ViewerMode::Image;
    }
    let mut head = [0u8; 8192];
    let n = File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    if looks_binary(&head[..n]) { ViewerMode::Hex } else { ViewerMode::Text }
}

/// Heuristic: NUL bytes, or many control characters outside UTF-8 text.
pub fn looks_binary(data: &[u8]) -> bool {
    if data.contains(&0) {
        return true;
    }
    if std::str::from_utf8(data).is_ok() {
        return false;
    }
    let control = data.iter().filter(|&&b| b < 0x09 || (b > 0x0d && b < 0x20)).count();
    control * 10 > data.len()
}

/// File bytes loaded for viewing.
#[derive(Clone, Debug, Default)]
pub struct Document {
    pub data: Vec<u8>,
    pub truncated: bool,
    pub file_size: u64,
}

impl Document {
    pub fn load(path: &Path) -> io::Result<Document> {
        let file = File::open(path)?;
        let file_size = file.metadata()?.len();
        let mut data = Vec::with_capacity(file_size.min(VIEW_LIMIT) as usize);
        file.take(VIEW_LIMIT).read_to_end(&mut data)?;
        Ok(Document { truncated: file_size > VIEW_LIMIT, data, file_size })
    }

    /// Text lines (lossy UTF-8, tabs expanded to 4 spaces, CR stripped).
    pub fn text_lines(&self) -> Vec<String> {
        let text = String::from_utf8_lossy(&self.data);
        let mut lines: Vec<String> = text
            .split('\n')
            .map(|l| expand_tabs(l.strip_suffix('\r').unwrap_or(l)))
            .collect();
        if lines.last().is_some_and(|l| l.is_empty()) && lines.len() > 1 {
            lines.pop();
        }
        lines
    }

    pub fn hex_line_count(&self) -> usize {
        self.data.len().div_ceil(16).max(1)
    }

    /// One hex dump row: `00000010  48 65 6c 6c 6f 20 77 6f  72 6c 64 0a           |Hello world.|`.
    pub fn hex_line(&self, row: usize) -> String {
        hex_line(&self.data, row)
    }
}

fn expand_tabs(line: &str) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + 8);
    let mut col = 0;
    for ch in line.chars() {
        if ch == '\t' {
            let n = 4 - col % 4;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push(ch);
            col += 1;
        }
    }
    out
}

pub fn hex_line(data: &[u8], row: usize) -> String {
    let start = row * 16;
    let chunk = data.get(start..(start + 16).min(data.len())).unwrap_or(&[]);
    let mut s = format!("{start:08x}  ");
    for i in 0..16 {
        match chunk.get(i) {
            Some(b) => s.push_str(&format!("{b:02x} ")),
            None => s.push_str("   "),
        }
        if i == 7 {
            s.push(' ');
        }
    }
    s.push(' ');
    s.push('|');
    for b in chunk {
        s.push(if b.is_ascii_graphic() || *b == b' ' { *b as char } else { '.' });
    }
    s.push('|');
    s
}

/// Finds `needle` in `lines` after (`line`, `col`), wrapping around once.
/// Returns (line, byte column in the line).
pub fn find_text(lines: &[String], needle: &str, case_sensitive: bool, from: (usize, usize)) -> Option<(usize, usize)> {
    if needle.is_empty() || lines.is_empty() {
        return None;
    }
    let norm = |s: &str| if case_sensitive { s.to_string() } else { s.to_lowercase() };
    let needle = norm(needle);
    let n = lines.len();
    let first = from.0.min(n - 1);
    // From just after `from` to the end, then from the top back to the start line.
    for step in 0..=n {
        let li = (first + step) % n;
        let hay = norm(&lines[li]);
        let mut start = if step == 0 { from.1 + 1 } else { 0 };
        while start < hay.len() && !hay.is_char_boundary(start) {
            start += 1;
        }
        if start > hay.len() {
            continue;
        }
        if let Some(pos) = hay[start..].find(&needle) {
            return Some((li, start + pos));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        std::fs::write(r.join("a.txt"), "hello\tworld\r\nline2\n").unwrap();
        std::fs::write(r.join("b.bin"), [0u8, 1, 2, 3, 255]).unwrap();
        std::fs::write(r.join("c.png"), [0u8]).unwrap();
        assert_eq!(detect_mode(&r.join("a.txt")), ViewerMode::Text);
        assert_eq!(detect_mode(&r.join("b.bin")), ViewerMode::Hex);
        assert_eq!(detect_mode(&r.join("c.png")), ViewerMode::Image);
        assert_eq!(detect_mode(r), ViewerMode::Folder);

        let doc = Document::load(&r.join("a.txt")).unwrap();
        assert_eq!(doc.text_lines(), vec!["hello   world".to_string(), "line2".to_string()]);
        assert!(!doc.truncated);
    }

    #[test]
    fn hex_rows() {
        let data = b"Hello world, this is hex!";
        assert_eq!(
            hex_line(data, 0),
            "00000000  48 65 6c 6c 6f 20 77 6f  72 6c 64 2c 20 74 68 69  |Hello world, thi|"
        );
        assert_eq!(
            hex_line(data, 1),
            "00000010  73 20 69 73 20 68 65 78  21                       |s is hex!|"
        );
        let doc = Document { data: data.to_vec(), ..Default::default() };
        assert_eq!(doc.hex_line_count(), 2);
    }

    #[test]
    fn find_wraps() {
        let lines: Vec<String> = ["alpha beta", "gamma", "beta again"].iter().map(|s| s.to_string()).collect();
        assert_eq!(find_text(&lines, "beta", true, (0, 0)), Some((0, 6)));
        assert_eq!(find_text(&lines, "beta", true, (0, 6)), Some((2, 0)));
        assert_eq!(find_text(&lines, "beta", true, (2, 0)), Some((0, 6)), "wraps to the top");
        assert_eq!(find_text(&lines, "GAMMA", false, (0, 0)), Some((1, 0)));
        assert_eq!(find_text(&lines, "GAMMA", true, (0, 0)), None);
    }
}
