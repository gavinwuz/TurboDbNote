use std::ops::Range;

pub(super) fn containing_row(starts: &[usize], offset: usize, rows: usize) -> usize {
    starts
        .partition_point(|start| *start <= offset)
        .saturating_sub(1)
        .min(rows.saturating_sub(1))
}

pub(super) fn needs_shape(
    row: usize,
    viewport: &Range<usize>,
    bytes: &Range<usize>,
    endpoints: &[usize],
) -> bool {
    viewport.contains(&row)
        || endpoints
            .iter()
            .any(|offset| bytes.start <= *offset && *offset <= bytes.end)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indexed_rows_preserve_boundary_and_eof_geometry() {
        let starts = [0, 6, 16, 19];
        for (offset, row) in [(0, 0), (5, 0), (6, 1), (15, 1), (16, 2), (19, 2), (200, 2)] {
            assert_eq!(containing_row(&starts, offset, 3), row);
        }
        assert_eq!(containing_row(&[], 0, 0), 0);
    }
    #[test]
    fn long_line_shapes_only_viewport_and_selection_endpoints() {
        let viewport = 100..140;
        let endpoints = [5, 999_999, 500_000];
        let shaped: Vec<_> = (0..10_000)
            .filter(|row| needs_shape(*row, &viewport, &(row * 100..(row + 1) * 100), &endpoints))
            .collect();
        assert!(shaped.len() <= viewport.len() + endpoints.len() * 2);
        assert!(shaped.contains(&0) && shaped.contains(&9999) && shaped.contains(&5000));
        assert!(viewport.clone().all(|row| shaped.contains(&row)));
    }
}
