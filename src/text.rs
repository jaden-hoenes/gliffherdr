//! Fitting text into terminal columns, shared by `list` and the picker.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Display width of `s` in terminal columns.
#[must_use]
pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Shorten `s` to at most `max` columns, ending with `…` when something was cut.
#[must_use]
pub fn truncate(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > max {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// Pad `s` with spaces on the right to exactly `cols` columns (truncating first if longer).
#[must_use]
pub fn pad(s: &str, cols: usize) -> String {
    let t = truncate(s, cols);
    let fill = cols.saturating_sub(width(&t));
    format!("{t}{}", " ".repeat(fill))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_table() {
        let cases = [
            ("abc", 5, "abc"),
            ("abc", 3, "abc"),
            ("abcd", 3, "ab…"),
            ("abcd", 1, "…"),
            ("abcd", 0, ""),
            ("日本語", 6, "日本語"),
            ("日本語", 5, "日本…"),
            ("日本語", 4, "日…"),
            ("", 3, ""),
        ];
        for (s, max, want) in cases {
            assert_eq!(truncate(s, max), want, "{s:?} {max}");
            assert!(width(&truncate(s, max)) <= max);
        }
    }

    #[test]
    fn pad_fills_to_the_exact_width() {
        assert_eq!(pad("ab", 4), "ab  ");
        assert_eq!(pad("abcdef", 4), "abc…");
        assert_eq!(width(&pad("日本", 7)), 7);
    }
}
