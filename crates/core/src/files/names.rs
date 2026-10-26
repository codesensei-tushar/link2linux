//! File names, in both directions.
//!
//! A received name comes from the other device, so it is never used as a
//! path. [`received`] keeps only a name: it replaces the characters that
//! cannot be stored, or would change what the user sees, and drops leading
//! dots, which would hide the file. [`numbered`] then gives it a number
//! when a file of that name already exists, as `photo (1).jpg`.
//!
//! [`outgoing`] turns a local file name into one the protocol accepts.

use std::ffi::OsStr;

/// Longest name the protocol carries, and most Linux file systems store,
/// in bytes.
pub(crate) const MAX_NAME_BYTES: usize = 255;

/// The name of a received file that has nothing usable left.
const FALLBACK: &str = "file";

/// The longest extension kept apart when a name is shortened or numbered,
/// in bytes. A longer one is just part of the name.
const MAX_EXTENSION_BYTES: usize = 32;

/// A safe name for a file received as `name`: no directories, nothing
/// hidden, at most [`MAX_NAME_BYTES`] bytes.
pub(crate) fn received(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| if is_unsafe(c) { '_' } else { c })
        .collect();
    let trimmed = replaced
        .trim_start_matches(|c: char| c == '.' || c.is_whitespace())
        .trim_end();
    numbered(
        if trimmed.is_empty() {
            FALLBACK
        } else {
            trimmed
        },
        0,
    )
}

/// `name` with number `n` before its extension, or `name` itself for 0,
/// shortened to [`MAX_NAME_BYTES`] bytes without cutting the extension.
pub(crate) fn numbered(name: &str, n: u32) -> String {
    let (stem, extension) = split_extension(name);
    let suffix = if n == 0 {
        String::new()
    } else {
        format!(" ({n})")
    };
    let room = MAX_NAME_BYTES - suffix.len() - extension.len();
    format!("{}{suffix}{extension}", truncate(stem, room))
}

/// The name a local file goes out with: its own, made valid UTF-8 and cut
/// to [`MAX_NAME_BYTES`] bytes. The receiver makes it safe for itself.
pub(crate) fn outgoing(name: &OsStr) -> String {
    let name = name.to_string_lossy();
    if name.is_empty() || name == "." || name == ".." {
        return FALLBACK.to_owned();
    }
    numbered(&name, 0)
}

/// The MIME type of a file named `name`, guessed from its extension for
/// common types, or empty.
pub(crate) fn mime_type(name: &str) -> &'static str {
    let (_, extension) = split_extension(name);
    match extension
        .trim_start_matches('.')
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "ogg" => "audio/ogg",
        "opus" => "audio/opus",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "apk" => "application/vnd.android.package-archive",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "",
    }
}

/// Whether `c` cannot be stored in a name (`/`, NUL), is a separator on
/// other systems, or would change how the name looks: control characters
/// and the marks that reorder text, which can make `exe.jpg` show as
/// `gpj.exe`.
fn is_unsafe(c: char) -> bool {
    c == '/'
        || c == '\\'
        || c.is_control()
        || matches!(
            c,
            '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
        )
}

/// Splits `name` into its stem and its extension with the dot, such as
/// `("photo", ".jpg")`. A name without a stem or with a long extension has
/// none.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if dot > 0 && name.len() - dot <= MAX_EXTENSION_BYTES && dot + 1 < name.len() => {
            name.split_at(dot)
        }
        _ => (name, ""),
    }
}

/// `text` cut to at most `max` bytes, on a character boundary.
fn truncate(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStrExt;

    use super::*;

    #[test]
    fn directories_and_unsafe_characters_never_survive() {
        assert_eq!(received("photo.jpg"), "photo.jpg");
        assert_eq!(received("../../.bashrc"), "_.._.bashrc");
        assert_eq!(received("/etc/passwd"), "_etc_passwd");
        assert_eq!(received("a\\b"), "a_b");
        assert_eq!(received("nul\0and\nnewline\u{7f}"), "nul_and_newline_");
        assert_eq!(received("c1\u{85}control"), "c1_control");
        // Reordering marks would show this as "photo_gpj.exe".
        assert_eq!(received("photo\u{202E}gpj.exe"), "photo_gpj.exe");
        assert_eq!(received("\u{FEFF}bom"), "_bom");
    }

    #[test]
    fn nothing_is_hidden_or_empty() {
        assert_eq!(received(".hidden"), "hidden");
        assert_eq!(received("...dots"), "dots");
        assert_eq!(received(". .x"), "x");
        assert_eq!(received("."), "file");
        assert_eq!(received(".."), "file");
        assert_eq!(received(""), "file");
        assert_eq!(received("   "), "file");
        assert_eq!(received("  spaced  "), "spaced");
    }

    #[test]
    fn long_names_are_cut_on_a_character_boundary_keeping_the_extension() {
        let long = format!("{}.jpg", "é".repeat(200));
        let name = received(&long);
        assert!(name.len() <= MAX_NAME_BYTES);
        assert!(name.ends_with(".jpg"));
        assert_eq!(name.len(), 2 * 125 + 4);

        let numbered = numbered(&long, 12);
        assert!(numbered.len() <= MAX_NAME_BYTES, "{}", numbered.len());
        assert!(numbered.ends_with(" (12).jpg"));

        // A long "extension" is part of the name.
        let dotted = format!("a.{}", "b".repeat(300));
        assert_eq!(received(&dotted).len(), MAX_NAME_BYTES);
        assert!(received(&dotted).starts_with("a.b"));
    }

    #[test]
    fn numbers_go_before_the_extension() {
        assert_eq!(numbered("photo.jpg", 0), "photo.jpg");
        assert_eq!(numbered("photo.jpg", 1), "photo (1).jpg");
        assert_eq!(numbered("archive.tar.gz", 2), "archive.tar (2).gz");
        assert_eq!(numbered("README", 3), "README (3)");
        assert_eq!(numbered("trailing.", 1), "trailing. (1)");
    }

    #[test]
    fn common_types_are_guessed() {
        assert_eq!(mime_type("IMG_0001.JPG"), "image/jpeg");
        assert_eq!(mime_type("notes.txt"), "text/plain");
        assert_eq!(mime_type("archive.tar.gz"), "application/gzip");
        assert_eq!(mime_type("README"), "");
        assert_eq!(mime_type(".png"), "");
    }

    #[test]
    fn outgoing_names_are_valid_utf8_within_the_limit() {
        assert_eq!(outgoing(OsStr::new("report.pdf")), "report.pdf");
        assert_eq!(
            outgoing(OsStr::from_bytes(b"caf\xe9.txt")),
            "caf\u{FFFD}.txt"
        );
        let long = "x".repeat(300) + ".txt";
        let name = outgoing(OsStr::new(&long));
        assert_eq!(name.len(), MAX_NAME_BYTES);
        assert!(name.ends_with(".txt"));
        assert_eq!(outgoing(OsStr::new("")), "file");
    }
}
