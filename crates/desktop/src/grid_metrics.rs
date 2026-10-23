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
    estimate(rows, font, 54.)
}
pub fn data_widths(rows: &Rows, font: f32) -> Vec<f32> {
    estimate(rows, font, 104.)
}
fn estimate(rows: &Rows, font: f32, header_controls: f32) -> Vec<f32> {
    let unit = font.clamp(10., 32.) * 0.55;
    rows.columns
        .iter()
        .enumerate()
        .map(|(col, name)| {
            let header = cells(name) as f32 * unit + header_controls;
            let mut sizes = rows
                .values
                .iter()
                .take(32)
                .filter_map(|r| r.get(col))
                .map(|v| {
                    let value = v.as_deref().unwrap_or("NULL");
                    let numeric = value.len() > 3
                        && value
                            .bytes()
                            .take(64)
                            .all(|c| c.is_ascii_digit() || b"+-.".contains(&c));
                    cells(value) as f32 * unit
                        + 20.
                        + if numeric {
                            unit * (value.len().min(64) / 3) as f32
                        } else {
                            0.
                        }
                })
                .collect::<Vec<_>>();
            sizes.sort_by(f32::total_cmp);
            let sample = sizes
                .get(sizes.len().saturating_sub(1) * 9 / 10)
                .copied()
                .unwrap_or(0.);
            header
                .max(sample)
                .clamp(72., (280. * font.clamp(10., 32.) / 14.).clamp(180., 640.))
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(data[0] >= 100. && data[0] < data[1]);
        assert!(values[0] < values[1]);
        assert!(values[1] < values[2]);
        assert!(values.iter().all(|v| *v >= 72. && *v <= 280.));
        assert_eq!(rows.values[0][2].as_ref().unwrap().len(), 10000);
    }
}
