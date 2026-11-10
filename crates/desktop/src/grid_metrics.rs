//! Bounded initial sizing; explicit user widths always win.
use turbodbn_services::database::workbench::Rows;
use unicode_width::UnicodeWidthChar;
fn cells(value: &str) -> usize {
    value
        .chars()
        .take(64)
        .take_while(|c| *c != '\n' && *c != '\r')
        .map(|c| if c == '\t' { 4 } else { c.width().unwrap_or(0) })
        .sum()
}
pub fn widths(rows: &Rows, font: f32) -> Vec<f32> {
    estimate(rows, font, 54., false)
}
pub fn data_widths(rows: &Rows, font: f32) -> Vec<f32> {
    estimate(rows, font, 68., true)
}
fn estimate(rows: &Rows, font: f32, header_controls: f32, compact_header: bool) -> Vec<f32> {
    let unit = font.clamp(10., 32.) * 0.55;
    rows.columns
        .iter()
        .enumerate()
        .map(|(col, name)| {
            let header_cells = if compact_header {
                cells(name).min(4)
            } else {
                cells(name)
            };
            let header = header_cells as f32 * unit + header_controls;
            let mut sizes = rows
                .values
                .iter()
                .take(64)
                .filter_map(|r| r.get(col))
                .map(|v| {
                    let value = v.as_deref().unwrap_or("NULL");
                    cells(value) as f32 * unit + 20.
                })
                .collect::<Vec<_>>();
            sizes.sort_by(f32::total_cmp);
            let sample = sizes
                .get(sizes.len().saturating_sub(1) * 9 / 10)
                .copied()
                .unwrap_or(0.);
            header.max(sample).clamp(
                if compact_header {
                    (100. * font / 14.).clamp(80., 200.)
                } else {
                    64.
                },
                (280. * font.clamp(10., 32.) / 14.).clamp(180., 640.),
            )
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn long_headers_do_not_expand_short_data_but_long_values_still_fit() {
        let rows = Rows {
            columns: vec![
                "original_identity_type".into(),
                "original_identity_id".into(),
                "identity".into(),
            ],
            values: vec![vec![
                Some("0".into()),
                Some("1234567890123456789".into()),
                Some("第一大区（福建电信）（闽江十六区）".into()),
            ]],
        };
        let widths = data_widths(&rows, 14.);
        assert_eq!(widths[0], 100.);
        assert!(widths[1] > widths[0] && widths[1] < 180.);
        assert!(widths[2] > widths[1]);
        assert_eq!(rows.columns[0], "original_identity_type");
        assert_eq!(rows.values[0][0].as_deref(), Some("0"));
    }

    #[test]
    fn numeric_widths_follow_values_without_unused_grouping_space() {
        let rows = Rows {
            columns: vec!["id".into(), "count".into(), "value".into()],
            values: vec![vec![
                Some("1".into()),
                Some("1234567890123456789".into()),
                Some("这是较长的一段说明文字".into()),
            ]],
        };
        let widths = data_widths(&rows, 14.);
        assert!(widths[0] < widths[1]);
        assert!(widths[1] < 180.);
        assert!(widths[1] < widths[2]);
        assert_eq!(rows.values[0][1].as_deref(), Some("1234567890123456789"));
    }

    #[test]
    fn widths_fit_short_and_wide_values_without_touching_original_data() {
        let rows = Rows {
            columns: vec!["id".into(), "name".into(), "description".into()],
            values: vec![vec![
                Some("123".into()),
                Some("测试中文宽度".into()),
                Some("x".repeat(10000)),
            ]],
        };
        let values = widths(&rows, 14.);
        let data = data_widths(&rows, 14.);
        assert!(data[0] >= 80. && data[0] < data[1]);
        assert!(values[0] < values[1]);
        assert!(values[1] < values[2]);
        assert!(values.iter().all(|v| *v >= 64. && *v <= 280.));
        assert_eq!(rows.values[0][2].as_ref().unwrap().len(), 10000);
    }
}
