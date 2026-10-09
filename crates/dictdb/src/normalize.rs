//! Search key normalization.
//!
//! Keys are folded so that a query matches regardless of case, compatibility
//! forms (ligatures, full-width letters) and diacritics on Latin, Greek and
//! Cyrillic letters. Marks on other scripts are kept, because there they
//! change meaning (e.g. Japanese dakuten: か vs が).

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Normalizes `text` into the form stored in, and searched against, the key index.
pub fn normalize_key(text: &str) -> String {
    let mut folded = String::with_capacity(text.len());
    let mut last_base = '\0';
    for c in text.trim().nfkd() {
        if is_combining_mark(c) {
            if strips_marks(last_base) {
                continue;
            }
        } else {
            last_base = c;
        }
        folded.extend(c.to_lowercase());
    }
    collapse_whitespace(&folded).nfc().collect()
}

fn strips_marks(base: char) -> bool {
    matches!(base as u32,
        0x0000..=0x024F     // Latin, Latin-1, Latin Extended-A/B
        | 0x1E00..=0x1EFF   // Latin Extended Additional
        | 0x0370..=0x03FF   // Greek
        | 0x1F00..=0x1FFF   // Greek Extended
        | 0x0400..=0x052F) // Cyrillic
}

fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::normalize_key;

    #[test]
    fn folds_case_and_latin_diacritics() {
        assert_eq!(normalize_key("Café"), "cafe");
        assert_eq!(normalize_key("NAÏVE"), "naive");
        assert_eq!(normalize_key("Ångström"), "angstrom");
    }

    #[test]
    fn folds_compatibility_forms_and_whitespace() {
        assert_eq!(normalize_key("  ﬁne\t tuning "), "fine tuning");
        assert_eq!(normalize_key("ＡＢＣ"), "abc");
    }

    #[test]
    fn keeps_marks_that_change_meaning() {
        assert_eq!(normalize_key("が"), "が");
        assert_ne!(normalize_key("が"), normalize_key("か"));
    }
}
