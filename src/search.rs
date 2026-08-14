//! Finding a string in cue text.
//!
//! Case is folded per character, which is what a subtitle search wants: it
//! makes Latin queries case-insensitive and leaves Japanese — where the notion
//! doesn't apply — untouched. Positions are byte ranges into the *original*
//! text (never into a lowercased copy, whose byte offsets would not line up),
//! so callers can slice with them safely.

use std::ops::Range;

/// The lowercased characters of `needle`, ready for [`first_match`]/[`matches`].
fn folded(needle: &str) -> Vec<char> {
    needle.chars().flat_map(char::to_lowercase).collect()
}

/// The byte index just past `needle` if it starts at byte `pos` of `text`.
/// `pos` must be a char boundary; the returned index always is one.
fn match_at(text: &str, pos: usize, needle: &[char]) -> Option<usize> {
    let mut k = 0;
    for (i, c) in text[pos..].char_indices() {
        if k == needle.len() {
            return Some(pos + i);
        }
        for lc in c.to_lowercase() {
            if needle.get(k) != Some(&lc) {
                return None;
            }
            k += 1;
        }
    }
    (k == needle.len()).then_some(text.len())
}

/// Whether `text` contains `needle`, ignoring case. Stops at the first hit.
pub fn contains(text: &str, needle: &str) -> bool {
    let needle = folded(needle);
    !needle.is_empty() && scan(text, &needle).next().is_some()
}

/// Byte ranges of every occurrence of `needle` in `text`, ignoring case.
/// Empty when `needle` is empty, so an unset search highlights nothing.
pub fn matches(text: &str, needle: &str) -> Vec<Range<usize>> {
    let needle = folded(needle);
    if needle.is_empty() {
        return Vec::new();
    }
    scan(text, &needle).collect()
}

/// `text` with every occurrence of `needle` replaced by `with`, and how many
/// there were. The replacement goes in verbatim — case is folded to *find* a
/// word, never to re-case what takes its place.
pub fn replace_all(text: &str, needle: &str, with: &str) -> (String, usize) {
    let hits = matches(text, needle);
    if hits.is_empty() {
        return (text.to_string(), 0);
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for hit in &hits {
        out.push_str(&text[at..hit.start]);
        out.push_str(with);
        at = hit.end;
    }
    out.push_str(&text[at..]);
    (out, hits.len())
}

/// Non-overlapping occurrences, left to right.
fn scan<'a>(text: &'a str, needle: &'a [char]) -> impl Iterator<Item = Range<usize>> + 'a {
    let mut pos = 0;
    std::iter::from_fn(move || {
        while pos < text.len() {
            match match_at(text, pos, needle) {
                Some(end) => {
                    let hit = pos..end;
                    // A needle that folds to nothing would not advance; step on.
                    pos = end.max(pos + next_char_len(text, pos));
                    return Some(hit);
                }
                None => pos += next_char_len(text, pos),
            }
        }
        None
    })
}

fn next_char_len(text: &str, pos: usize) -> usize {
    text[pos..].chars().next().map_or(1, char::len_utf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found<'a>(text: &'a str, needle: &str) -> Vec<&'a str> {
        matches(text, needle)
            .into_iter()
            .map(|r| &text[r])
            .collect()
    }

    #[test]
    fn finds_every_occurrence() {
        assert_eq!(found("ここにも ここにも ある", "ここ"), ["ここ", "ここ"]);
        assert_eq!(found("abcabc", "bc"), ["bc", "bc"]);
    }

    #[test]
    fn ignores_case_without_disturbing_the_offsets() {
        let text = "Sentence with UTOR in it";
        assert_eq!(found(text, "utor"), ["UTOR"]);
        assert_eq!(found(text, "SENTENCE"), ["Sentence"]);
        // The range indexes the original text, not a lowercased copy.
        assert_eq!(matches(text, "utor"), vec![14..18]);
    }

    #[test]
    fn matches_are_not_overlapping_and_stay_on_char_boundaries() {
        assert_eq!(found("aaaa", "aa"), ["aa", "aa"]);
        // Multi-byte text: slicing the returned ranges must not panic.
        let text = "字幕字幕字幕";
        assert_eq!(found(text, "字幕"), ["字幕", "字幕", "字幕"]);
    }

    #[test]
    fn a_needle_longer_than_the_text_is_not_a_match() {
        assert!(!contains("あい", "あいうえお"));
        assert!(found("あい", "あいうえお").is_empty());
    }

    #[test]
    fn an_empty_needle_matches_nothing() {
        assert!(!contains("anything", ""));
        assert!(matches("anything", "").is_empty());
    }

    #[test]
    fn replaces_every_occurrence() {
        assert_eq!(
            replace_all("ビルドリはビルドリです", "ビルドリ", "UTOR"),
            ("UTORはUTORです".to_string(), 2)
        );
        // Case folds to find, but the replacement goes in as typed.
        assert_eq!(
            replace_all("Utor and UTOR", "utor", "UTOR"),
            ("UTOR and UTOR".to_string(), 2)
        );
        // A replacement containing the needle doesn't feed on itself.
        assert_eq!(
            replace_all("abc", "abc", "abcabc"),
            ("abcabc".to_string(), 1)
        );
        // Replacing with nothing deletes the word.
        assert_eq!(replace_all("あいうえお", "うえ", ""), ("あいお".to_string(), 1));
        // No match leaves the text exactly as it was.
        assert_eq!(replace_all("そのまま", "ない", "x"), ("そのまま".to_string(), 0));
    }

    #[test]
    fn contains_agrees_with_matches() {
        for (text, needle) in [
            ("プロジェクター", "ジェ"),
            ("プロジェクター", "ジェク"),
            ("プロジェクター", "クタ"),
            ("プロジェクター", "ない"),
            ("", "x"),
        ] {
            assert_eq!(
                contains(text, needle),
                !matches(text, needle).is_empty(),
                "{text} / {needle}"
            );
        }
    }
}
