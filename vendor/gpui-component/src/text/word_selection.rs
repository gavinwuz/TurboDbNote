use std::ops::Range;

// SQL identifiers keep underscores; punctuation and identifier quotes delimit words.
pub(super) fn identifier_range(text: &str, index: usize) -> Option<Range<usize>> {
    use unicode_segmentation::UnicodeSegmentation;
    let mut start = None;
    let mut end = 0;
    for (offset, grapheme) in text.grapheme_indices(true) {
        let word = grapheme
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        if word {
            start.get_or_insert(offset);
            end = offset + grapheme.len();
        } else if let Some(begin) = start.take() {
            if (begin..end).contains(&index) {
                return Some(begin..end);
            }
        }
    }
    start
        .filter(|begin| (*begin..end).contains(&index))
        .map(|begin| begin..end)
}

#[cfg(test)]
mod tests {
    use super::identifier_range;
    #[test]
    fn identifier_selection_preserves_sql_and_unicode() {
        for (text, index, expected) in [
            ("`original_lookface` varchar(500)", 4, "original_lookface"),
            ("table.column_2", 9, "column_2"),
            ("`用户_名称`", 5, "用户_名称"),
            ("'cafe\u{301}_name'", 5, "cafe\u{301}_name"),
        ] {
            assert_eq!(&text[identifier_range(text, index).unwrap()], expected);
        }
        assert_eq!(identifier_range("a.b", 1), None);
        assert_eq!(identifier_range("`name`", 0), None);
        assert_eq!(identifier_range("name", 4), None);
    }
}
