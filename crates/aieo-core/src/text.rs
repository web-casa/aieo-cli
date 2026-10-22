//! String helpers with JavaScript semantics. The readiness modules are held
//! to the TypeScript engine by conformance fixtures, so lengths are counted
//! in UTF-16 code units and "whitespace" is ECMAScript's `\s` set.

/// ECMAScript WhiteSpace + LineTerminator (the `\s` class and `trim()`).
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// `String.prototype.trim()`.
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

/// `.length` of a JavaScript string.
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// `s.slice(0, n)` with `n` in UTF-16 code units (never splits a character:
/// a surrogate pair cut in half is dropped, as a lone surrogate would be
/// unrepresentable here).
pub fn js_prefix(s: &str, n: usize) -> &str {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        units += c.len_utf16();
        if units > n {
            return &s[..i];
        }
    }
    s
}

/// `s.replace(/\s+/g, " ").trim()`.
pub fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending = false;
    for c in s.chars() {
        if is_js_space(c) {
            pending = true;
        } else {
            if pending && !out.is_empty() {
                out.push(' ');
            }
            pending = false;
            out.push(c);
        }
    }
    out
}

/// `String.prototype.toLowerCase()` (Unicode default case conversion).
pub fn js_lower(s: &str) -> String {
    s.to_lowercase()
}

/// 1-based line of a byte offset.
pub fn line_at(text: &str, byte: usize) -> u32 {
    let end = byte.min(text.len());
    1 + text.as_bytes()[..end]
        .iter()
        .filter(|&&b| b == b'\n')
        .count() as u32
}

/// 1-based line of the first occurrence of `needle`, if any.
pub fn line_of(text: &str, needle: &str) -> Option<u32> {
    if needle.is_empty() {
        return None;
    }
    text.find(needle).map(|i| line_at(text, i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_semantics() {
        assert_eq!(js_trim("\u{FEFF} a \u{3000}"), "a");
        // NEL is Unicode whitespace but not ECMAScript whitespace
        assert_eq!(js_trim("\u{85}a"), "\u{85}a");
        assert_eq!(utf16_len("a😀中"), 4);
        assert_eq!(js_prefix("a😀b", 2), "a");
        assert_eq!(js_prefix("a😀b", 3), "a😀");
        assert_eq!(collapse_ws("  a \n\t b  "), "a b");
        assert_eq!(line_of("a\nb\nc", "c"), Some(3));
    }
}
