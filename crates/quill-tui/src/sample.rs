//! A small PDF, generated here, for trying the host's preview pane before
//! messages have attachments (M4). Invented content only.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

pub const FILE_NAME: &str = "Quill sample.pdf";

/// Writes the sample into `dir`, a directory private to this user that is
/// created if need be.
pub fn write(dir: &Path, path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    let mut file = fs::File::create(path)?;
    file.write_all(&pdf())
}

/// A one-page PDF with a heading and a few lines of text.
pub fn pdf() -> Vec<u8> {
    let lines = [
        "This page was written by Quill's core and is shown by the app shell.",
        "",
        "The core asked for it over the bridge with a pane.preview message.",
        "The shell opened it in a Quick Look pane beside the terminal.",
        "",
        "Attachments will arrive here the same way in a later milestone.",
    ];
    let mut content = String::from("BT\n/F1 30 Tf\n72 700 Td\n(Quill) Tj\n/F2 13 Tf\n0 -34 Td\n");
    for line in lines {
        content.push_str(&format!("({}) Tj\n0 -20 Td\n", escape(line)));
    }
    content.push_str("ET\n");

    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
         /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> >>"
            .to_owned(),
        format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>".to_owned(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
    ];

    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    // Each cross-reference entry is exactly twenty bytes.
    out.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// Escapes the characters that end or continue a PDF string.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '(' | ')' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    #[test]
    fn the_cross_reference_table_points_at_every_object() {
        let pdf = pdf();
        assert!(pdf.starts_with(b"%PDF-1.4\n"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        let text = String::from_utf8(pdf.clone()).unwrap();

        let xref = find(&pdf, b"xref\n").unwrap();
        let declared: usize = text
            .rsplit("startxref\n")
            .next()
            .and_then(|tail| tail.lines().next())
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(declared, xref);

        let entries: Vec<&str> = text[xref..].lines().skip(2).take(7).collect();
        assert_eq!(entries[0], "0000000000 65535 f ");
        for (index, entry) in entries.iter().enumerate().skip(1) {
            assert_eq!(entry.len(), 19, "{entry:?}");
            let offset: usize = entry[..10].parse().unwrap();
            let header = format!("{index} 0 obj\n");
            assert!(
                pdf[offset..].starts_with(header.as_bytes()),
                "object {index} is not at {offset}"
            );
        }
    }

    #[test]
    fn the_stream_length_is_exact() {
        let text = String::from_utf8(pdf()).unwrap();
        let declared: usize = text
            .split("/Length ")
            .nth(1)
            .and_then(|tail| tail.split(' ').next())
            .unwrap()
            .parse()
            .unwrap();
        let start = text.find("stream\n").unwrap() + "stream\n".len();
        let end = text.find("endstream").unwrap();
        assert_eq!(end - start, declared);
    }

    #[test]
    fn parentheses_and_backslashes_are_escaped() {
        assert_eq!(escape(r"a (b) \ c"), r"a \(b\) \\ c");
    }

    #[test]
    fn the_file_lands_in_a_private_directory() {
        let parent = std::env::temp_dir().join(format!("quill-sample-test-{}", std::process::id()));
        let dir = parent.join("inner");
        let path = dir.join(FILE_NAME);
        write(&dir, &path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), pdf());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        fs::remove_dir_all(&parent).unwrap();
    }
}
