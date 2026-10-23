use std::ops::Range;

// Query only the relevant snapshot intervals, then map bounded pending edits forward.
pub(super) fn mapped_styles<T: Copy + Default>(
    styles: &[(Range<usize>, T)],
    edits: &[(Range<usize>, usize)],
    range: Range<usize>,
) -> Vec<(Range<usize>, T)> {
    let mut old = range.clone();
    for (edit, inserted) in edits.iter().rev() {
        let undo = |position: usize, end: bool| {
            if position <= edit.start {
                position
            } else if position >= edit.start + inserted {
                position - inserted + edit.len()
            } else if end {
                edit.end
            } else {
                edit.start
            }
        };
        old = undo(old.start, false)..undo(old.end, true);
    }
    let start = styles.partition_point(|(r, _)| r.end <= old.start);
    let mut result: Vec<(Range<usize>, T)> = styles[start..]
        .iter()
        .take_while(|(r, _)| r.start < old.end)
        .cloned()
        .collect();
    for (edit, inserted) in edits {
        let mut next = vec![];
        for (r, style) in result {
            if r.start < edit.start {
                next.push((r.start..r.end.min(edit.start), style));
            }
            if r.end > edit.end {
                let shift = |position: usize| position - edit.end + edit.start + inserted;
                next.push((shift(r.start.max(edit.end))..shift(r.end), style));
            }
        }
        if *inserted > 0 {
            next.push((edit.start..edit.start + inserted, T::default()));
        }
        result = next;
    }
    result.sort_by_key(|(r, _)| r.start);
    let mut complete = vec![];
    let mut cursor = range.start;
    for (r, style) in result {
        let start = r.start.max(cursor);
        let end = r.end.min(range.end);
        if start >= end {
            continue;
        }
        if cursor < start {
            complete.push((cursor..start, T::default()));
        }
        complete.push((start..end, style));
        cursor = end;
    }
    if cursor < range.end {
        complete.push((cursor..range.end, T::default()));
    }
    complete
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_edits_preserve_unaffected_styles_and_cover_new_unicode_text() {
        let colored = Some(7_u8);
        let original = vec![(0..6, colored), (6..7, None), (7..12, colored)];
        for (edits, length) in [
            (vec![(12..12, 6)], 18),
            (vec![(0..0, 6), (13..18, 0)], 13),
            (vec![(7..12, 0), (7..7, 9), (7..10, 0)], 13),
            (vec![(0..12, 0)], 0),
        ] {
            let actual = mapped_styles(&original, &edits, 0..length);
            let mut cursor = 0;
            for (r, _) in &actual {
                assert_eq!(r.start, cursor);
                cursor = r.end;
            }
            assert_eq!(cursor, length);
        }
        let actual = mapped_styles(&original, &[(12..12, 6)], 0..18);
        assert_eq!(actual[0].1, colored);
        assert_eq!(actual.last().unwrap().0, 12..18);
        let visible = mapped_styles(&original, &[(0..0, 6)], 13..18);
        assert_eq!(visible[0].0, 13..18);
        assert_eq!(visible[0].1, colored);
    }
}
