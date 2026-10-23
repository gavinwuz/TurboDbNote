use std::{ops::Range, rc::Rc};

use gpui::{
    App, Bounds, Font, LineFragment, Pixels, Point, ShapedLine, Size, Window, point, px, size,
};
use ropey::Rope;
use smallvec::SmallVec;

use super::geometry_index::containing_row;
use crate::input::RopeExt;

/// A line with soft wrapped lines info.
#[derive(Debug, Clone)]
pub(super) struct LineItem {
    /// The original line text, without end `\n`.
    line: Rope,
    /// The soft wrapped lines relative byte range (0..line.len) of this line (Include first line).
    ///
    /// Not contains the line end `\n`.
    pub(super) wrapped_lines: Vec<Range<usize>>,
}

impl LineItem {
    /// Get the bytes length of this line.
    #[inline]
    pub(super) fn len(&self) -> usize {
        self.line.len()
    }

    /// Get number of soft wrapped lines of this line (include the first line).
    #[inline]
    pub(super) fn lines_len(&self) -> usize {
        self.wrapped_lines.len()
    }

    /// Get the height of this line item with given line height.
    pub(super) fn height(&self, line_height: Pixels) -> Pixels {
        self.lines_len() as f32 * line_height
    }
}

#[derive(Debug, Default)]
pub(super) struct LongestRow {
    /// The 0-based row index.
    pub row: usize,
    /// The bytes length of the longest line.
    pub len: usize,
}

/// Used to prepare the text with soft wrap to be get lines to displayed in the Editor.
///
/// After use lines to calculate the scroll size of the Editor.
pub(super) struct TextWrapper {
    text: Rope,
    /// Total wrapped lines (Inlucde the first line), value is start and end index of the line.
    soft_lines: usize,
    row_tops: Vec<usize>,
    font: Font,
    font_size: Pixels,
    /// If is none, it means the text is not wrapped
    pub(super) wrap_width: Option<Pixels>,
    /// The longest (row, bytes len) in characters, used to calculate the horizontal scroll width.
    pub(super) longest_row: LongestRow,
    /// The lines by split \n
    pub(super) lines: Vec<LineItem>,

    _initialized: bool,
    pub(super) incremental_wrap: bool,
}

#[allow(unused)]
impl TextWrapper {
    pub(super) fn new(font: Font, font_size: Pixels, wrap_width: Option<Pixels>) -> Self {
        Self {
            text: Rope::new(),
            font,
            font_size,
            wrap_width,
            soft_lines: 0,
            row_tops: vec![0],
            longest_row: LongestRow::default(),
            lines: Vec::new(),
            _initialized: false,
            incremental_wrap: false,
        }
    }

    #[inline]
    pub(super) fn set_default_text(&mut self, text: &Rope) {
        self.text = text.clone();
    }

    /// Get the total number of lines including wrapped lines.
    #[inline]
    pub(super) fn len(&self) -> usize {
        self.soft_lines
    }

    pub(super) fn row_top(&self, row: usize) -> usize {
        self.row_tops
            .get(row)
            .copied()
            .filter(|_| self.row_tops.len() == self.lines.len() + 1)
            .unwrap_or_else(|| self.lines.iter().take(row).map(LineItem::lines_len).sum())
    }

    pub(super) fn row_for_display_line(&self, soft_row: usize) -> usize {
        if self.row_tops.len() == self.lines.len() + 1 {
            return containing_row(&self.row_tops, soft_row, self.lines.len());
        }
        (0..self.lines.len())
            .find(|row| self.row_top(row + 1) > soft_row)
            .unwrap_or_else(|| self.lines.len().saturating_sub(1))
    }

    /// Get the line item by row index.
    #[inline]
    pub(super) fn line(&self, row: usize) -> Option<&LineItem> {
        self.lines.iter().skip(row).next()
    }

    pub(super) fn set_wrap_width(&mut self, wrap_width: Option<Pixels>, cx: &mut App) {
        if wrap_width == self.wrap_width {
            return;
        }

        self.wrap_width = wrap_width;
        self.update_all(&self.text.clone(), cx);
    }

    pub(super) fn set_font(&mut self, font: Font, font_size: Pixels, cx: &mut App) {
        if self.font.eq(&font) && self.font_size == font_size {
            return;
        }

        self.font = font;
        self.font_size = font_size;
        self.update_all(&self.text.clone(), cx);
    }

    pub(super) fn prepare_if_need(&mut self, text: &Rope, cx: &mut App) {
        if self._initialized {
            return;
        }
        self._initialized = true;
        self.update_all(text, cx);
    }

    /// Update the text wrapper and recalculate the wrapped lines.
    ///
    /// If the `text` is the same as the current text, do nothing.
    ///
    /// - `changed_text`: The text [`Rope`] that has changed.
    /// - `range`: The `selected_range` before change.
    /// - `new_text`: The inserted text.
    /// - `force`: Whether to force the update, if false, the update will be skipped if the text is the same.
    /// - `cx`: The application context.
    pub(super) fn update(
        &mut self,
        changed_text: &Rope,
        range: &Range<usize>,
        new_text: &Rope,
        cx: &mut App,
    ) {
        let mut line_wrapper = cx
            .text_system()
            .line_wrapper(self.font.clone(), self.font_size);
        self._update(
            changed_text,
            range,
            new_text,
            &mut |line_str, wrap_width| {
                line_wrapper
                    .wrap_line(&[LineFragment::text(line_str)], wrap_width)
                    .collect()
            },
        );
    }

    fn _update<F>(
        &mut self,
        changed_text: &Rope,
        range: &Range<usize>,
        new_text: &Rope,
        wrap_line: &mut F,
    ) where
        F: FnMut(&str, Pixels) -> Vec<gpui::Boundary>,
    {
        // Remove the old changed lines.
        let start_row = self.text.offset_to_point(range.start).row;
        let start_row = start_row.min(self.lines.len().saturating_sub(1));
        let end_row = self.text.offset_to_point(range.end).row;
        let end_row = end_row.min(self.lines.len().saturating_sub(1));
        let rows_range = start_row..=end_row;

        if rows_range.contains(&self.longest_row.row) {
            self.longest_row = LongestRow::default();
        }

        let mut longest_row_ix = self.longest_row.row;
        let mut longest_row_len = self.longest_row.len;

        // To add the new lines.
        let new_start_row = changed_text.offset_to_point(range.start).row;
        let new_start_offset = changed_text.line_start_offset(new_start_row);
        let new_end_row = changed_text
            .offset_to_point(range.start + new_text.len())
            .row;
        let new_end_offset = changed_text.line_end_offset(new_end_row);
        let new_range = new_start_offset..new_end_offset;

        let mut new_lines = vec![];
        let wrap_width = self.wrap_width;

        // line not contains `\n`.
        for (ix, line) in Rope::from(changed_text.slice(new_range))
            .iter_lines()
            .enumerate()
        {
            let line_str = line.to_string();
            let mut wrapped_lines = vec![];
            let mut prev_boundary_ix = 0;
            if self.incremental_wrap
                && ix == 0
                && start_row == new_start_row
                && self.wrap_width.is_some()
            {
                let changed_column = range
                    .start
                    .saturating_sub(self.text.line_start_offset(start_row));
                if let Some(previous) = self.lines.get(start_row) {
                    // Reflow at least the previous soft line, preserving word-boundary context.
                    let keep = previous
                        .wrapped_lines
                        .iter()
                        .take_while(|part| part.end < changed_column)
                        .count()
                        .saturating_sub(1);
                    wrapped_lines.extend(previous.wrapped_lines.iter().take(keep).cloned());
                    prev_boundary_ix = wrapped_lines.last().map_or(0, |part| part.end);
                    if prev_boundary_ix > line_str.len()
                        || !line_str.is_char_boundary(prev_boundary_ix)
                    {
                        wrapped_lines.clear();
                        prev_boundary_ix = 0;
                    }
                }
            }

            if line_str.len() > longest_row_len {
                longest_row_ix = new_start_row + ix;
                longest_row_len = line_str.len();
            }

            // If wrap_width is Pixels::MAX, skip wrapping to disable word wrap
            if let Some(wrap_width) = wrap_width {
                // Here only have wrapped line, if there is no wrap meet, the `line_wraps` result will empty.
                let base = prev_boundary_ix;
                for boundary in wrap_line(&line_str[base..], wrap_width) {
                    wrapped_lines.push(prev_boundary_ix..base + boundary.ix);
                    prev_boundary_ix = base + boundary.ix;
                }
            }

            // Reset of the line
            if !line_str[prev_boundary_ix..].is_empty() || prev_boundary_ix == 0 {
                wrapped_lines.push(prev_boundary_ix..line.len());
            }

            new_lines.push(LineItem {
                line: Rope::from(line),
                wrapped_lines,
            });
        }

        if self.lines.len() == 0 {
            self.lines = new_lines;
        } else {
            self.lines.splice(rows_range, new_lines);
        }

        self.text = changed_text.clone();
        self.row_tops.truncate(start_row + 1);
        for line in &self.lines[start_row..] {
            self.row_tops
                .push(self.row_tops.last().copied().unwrap_or(0) + line.lines_len());
        }
        self.soft_lines = self.row_tops.last().copied().unwrap_or(0);
        self.longest_row = LongestRow {
            row: longest_row_ix,
            len: longest_row_len,
        }
    }

    /// Update the text wrapper and recalculate the wrapped lines.
    ///
    /// If the `text` is the same as the current text, do nothing.
    fn update_all(&mut self, text: &Rope, cx: &mut App) {
        self.update(text, &(0..text.len()), &text, cx);
    }

    /// Return display point (with soft wrap) from the given byte offset in the text.
    ///
    /// Panics if the `offset` is out of bounds.
    pub(crate) fn offset_to_display_point(&self, offset: usize) -> DisplayPoint {
        let row = self.text.offset_to_point(offset).row;
        let start = self.text.line_start_offset(row);
        let line = &self.lines[row];

        let wrapped_row = self.row_top(row);

        let local_offset = offset.saturating_sub(start);
        let ix = line
            .wrapped_lines
            .partition_point(|part| part.end <= local_offset)
            .min(line.wrapped_lines.len().saturating_sub(1));
        if let Some(range) = line.wrapped_lines.get(ix) {
            if range.contains(&local_offset) {
                return DisplayPoint::new(
                    wrapped_row + ix,
                    ix,
                    local_offset.saturating_sub(range.start),
                );
            }
        }

        // Otherwise return the eof of the line.
        let last_range = line.wrapped_lines.last().unwrap_or(&(0..0));
        let ix = line.lines_len().saturating_sub(1);
        return DisplayPoint::new(wrapped_row + ix, ix, last_range.len());
    }

    /// Return byte offset in the text from the given display point (with soft wrap).
    ///
    /// Panics if the `point.row` is out of bounds.
    pub(crate) fn display_point_to_offset(&self, point: DisplayPoint) -> usize {
        let row = self.row_for_display_line(point.row);
        let wrapped_row = self.row_top(row);
        if let Some(line) = self.lines.get(row) {
            if wrapped_row + line.lines_len() > point.row {
                let line_start = self.text.line_start_offset(row);
                let local_row = point.row.saturating_sub(wrapped_row);
                if let Some(range) = line.wrapped_lines.get(local_row) {
                    return line_start + (range.start + point.column).min(range.end);
                } else {
                    // If not found, return the end of the line.
                    return line_start + line.len();
                }
            }
        }

        return self.text.len();
    }

    pub(crate) fn display_point_to_point(&self, point: DisplayPoint) -> tree_sitter::Point {
        let offset = self.display_point_to_offset(point);
        self.text.offset_to_point(offset)
    }

    pub(crate) fn point_to_display_point(&self, point: tree_sitter::Point) -> DisplayPoint {
        let offset = self.text.point_to_offset(point);
        self.offset_to_display_point(offset)
    }
}

/// The actually display point in the text.
///
/// This is usually used to describe the
/// position in the text with `soft-wrap` mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayPoint {
    /// The 0-based soft wrapped row index in the text.
    pub row: usize,
    /// The 0-based row index in local line (include first line).
    ///
    /// This value only valid when return from [`TextWrapper::offset_to_display_point`], otherwise it will be ignored.
    pub local_row: usize,
    /// The 0-based column byte index in the display line (with soft wrap).
    pub column: usize,
}

impl DisplayPoint {
    pub fn new(row: usize, local_row: usize, column: usize) -> Self {
        Self {
            row,
            local_row,
            column,
        }
    }
}

/// The layout info of a line with soft wrapped lines.
#[derive(Clone)]
pub(crate) struct LineLayout {
    /// Total bytes length of this line.
    len: usize,
    /// The soft wrapped lines of this line (Include the first line).
    pub(crate) wrapped_lines: Rc<SmallVec<[ShapedLine; 1]>>,
    offsets: Rc<Vec<usize>>,
    pub(crate) longest_width: Pixels,
}

impl LineLayout {
    pub(crate) fn new() -> Self {
        Self {
            len: 0,
            longest_width: px(0.),
            wrapped_lines: Rc::new(SmallVec::new()),
            offsets: Rc::new(vec![0]),
        }
    }

    pub(crate) fn lines(mut self, wrapped_lines: SmallVec<[ShapedLine; 1]>) -> Self {
        self.set_wrapped_lines(wrapped_lines);
        self
    }

    pub(crate) fn set_wrapped_lines(&mut self, wrapped_lines: SmallVec<[ShapedLine; 1]>) {
        self.len = wrapped_lines.iter().map(|l| l.len).sum();
        let width = wrapped_lines
            .iter()
            .map(|l| l.width)
            .max()
            .unwrap_or_default();
        self.longest_width = width;
        let mut offsets = Vec::with_capacity(wrapped_lines.len() + 1);
        offsets.push(0);
        for line in &wrapped_lines {
            offsets.push(offsets.last().copied().unwrap_or(0) + line.len);
        }
        self.offsets = Rc::new(offsets);
        self.wrapped_lines = Rc::new(wrapped_lines);
    }

    #[inline]
    pub(super) fn len(&self) -> usize {
        self.len
    }

    /// Get the position (x, y) for the given index in this line layout.
    ///
    /// - The `offset` is a local byte index in this line layout.
    /// - The return value is relative to the top-left corner of this line layout, start from (0, 0)
    pub(crate) fn position_for_index(
        &self,
        offset: usize,
        line_height: Pixels,
    ) -> Option<Point<Pixels>> {
        let i = containing_row(&self.offsets, offset, self.wrapped_lines.len());
        if let Some(line) = self.wrapped_lines.get(i) {
            let acc_len = self.offsets[i];
            let is_last = i + 1 == self.wrapped_lines.len();
            let line_len = if is_last { line.len + 1 } else { line.len };

            let range = acc_len..(acc_len + line_len);
            if range.contains(&offset) {
                let x = line.x_for_index(offset.saturating_sub(acc_len));
                return Some(point(x, i * line_height));
            }
        }

        None
    }

    /// Get the closest index for the given x in this line layout.
    pub(super) fn closest_index_for_x(&self, x: Pixels) -> usize {
        let mut acc_len = 0;
        for (i, line) in self.wrapped_lines.iter().enumerate() {
            let is_last = i + 1 == self.wrapped_lines.len();
            if x <= line.width {
                let mut ix = line.closest_index_for_x(x);
                if !is_last && ix == line.text.len() {
                    // For soft wrap line, we can't put the cursor at the end of the line.
                    let c_len = line.text.chars().last().map(|c| c.len_utf8()).unwrap_or(0);
                    ix = ix.saturating_sub(c_len);
                }

                return acc_len + ix;
            }
            acc_len += line.text.len();
        }

        acc_len
    }

    /// Get the index for the given position (x, y) in this line layout.
    ///
    /// The `pos` is relative to the top-left corner of this line layout, start from (0, 0)
    /// The return value is a local byte index in this line layout, start from 0.
    pub(super) fn closest_index_for_position(
        &self,
        pos: Point<Pixels>,
        line_height: Pixels,
    ) -> Option<usize> {
        if pos.y < px(0.) {
            return None;
        }
        let row = (f32::from(pos.y) / f32::from(line_height).max(1.)).floor() as usize;
        let line = self.wrapped_lines.get(row)?;
        let mut ix = line.closest_index_for_x(pos.x);
        if row + 1 < self.wrapped_lines.len() && ix == line.text.len() {
            ix = ix.saturating_sub(line.text.chars().last().map_or(0, char::len_utf8));
        }
        Some(self.offsets[row] + ix)
    }

    pub(super) fn index_for_position(
        &self,
        pos: Point<Pixels>,
        line_height: Pixels,
    ) -> Option<usize> {
        if pos.y < px(0.) {
            return None;
        }
        let row = (f32::from(pos.y) / f32::from(line_height).max(1.)).floor() as usize;
        let line = self.wrapped_lines.get(row)?;
        Some(self.offsets[row] + line.index_for_x(pos.x)?)
    }

    pub(super) fn size(&self, line_height: Pixels) -> Size<Pixels> {
        size(self.longest_width, self.wrapped_lines.len() * line_height)
    }

    pub(super) fn paint(
        &self,
        pos: Point<Pixels>,
        line_height: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.paint_range(pos, line_height, 0..self.wrapped_lines.len(), window, cx);
    }
    pub(super) fn paint_visible(
        &self,
        pos: Point<Pixels>,
        line_height: Pixels,
        clip: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let height = f32::from(line_height).max(1.);
        let start =
            ((f32::from(clip.top() - pos.y) / height).floor().max(0.) as usize).saturating_sub(1);
        let end = ((f32::from(clip.bottom() - pos.y) / height).ceil().max(0.) as usize)
            .saturating_add(1)
            .min(self.wrapped_lines.len());
        self.paint_range(pos, line_height, start..end, window, cx);
    }
    fn paint_range(
        &self,
        pos: Point<Pixels>,
        line_height: Pixels,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) {
        for (ix, line) in self
            .wrapped_lines
            .iter()
            .enumerate()
            .skip(range.start)
            .take(range.end.saturating_sub(range.start))
        {
            _ = line.paint(
                pos + point(px(0.), ix * line_height),
                line_height,
                window,
                cx,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Boundary, FontFeatures, FontStyle, FontWeight, px};

    #[test]
    fn line_offsets_keep_offscreen_geometry_without_shaping() {
        let mut line = LineLayout::new();
        line.set_wrapped_lines(smallvec::smallvec![
            ShapedLine::default().with_len(100),
            ShapedLine::default().with_len(50),
        ]);
        for offset in [0, 99, 100, 149, 150] {
            let pos = line.position_for_index(offset, px(22.)).unwrap();
            assert_eq!(pos.y, if offset < 100 { px(0.) } else { px(22.) });
        }
        assert!(line.position_for_index(151, px(22.)).is_none());
        assert_eq!(line.size(px(22.)).height, px(44.));
    }
    #[test]
    fn row_index_survives_line_insert_delete_and_unicode_edits() {
        let font = Font {
            family: "Arial".into(),
            weight: FontWeight::default(),
            style: FontStyle::Normal,
            features: FontFeatures::default(),
            fallbacks: None,
        };
        let mut wrapper = TextWrapper::new(font, px(14.), None);
        let mut text = Rope::from("SELECT 中文\nsecond\nthird");
        let mut no_wrap = |_: &str, _: Pixels| vec![];
        wrapper._update(&text, &(0..text.len()), &text, &mut no_wrap);
        for (range, inserted) in [
            (0..0, "first\n"),
            (6..19, "new\nline"),
            (0..5, ""),
            (0..0, "中文\n"),
        ] {
            text.replace(range.clone(), inserted);
            wrapper._update(&text, &range, &Rope::from(inserted), &mut no_wrap);
            for row in 0..wrapper.lines.len() {
                assert_eq!(wrapper.row_top(row), row);
                assert_eq!(wrapper.row_for_display_line(row), row);
            }
            assert_eq!(wrapper.len(), wrapper.lines.len());
        }
    }
    #[test]
    fn test_update() {
        let font = gpui::Font {
            family: "Arial".into(),
            weight: FontWeight::default(),
            style: FontStyle::Normal,
            features: FontFeatures::default(),
            fallbacks: None,
        };

        let mut wrapper = TextWrapper::new(font, px(14.), None);
        let mut text = Rope::from(
            "Hello, 世界!\r\nThis is second line.\nThis is third line.\n这里是第 4 行。",
        );

        fn fake_wrap_line(_line: &str, _wrap_width: Pixels) -> Vec<Boundary> {
            vec![]
        }

        #[track_caller]
        fn assert_wrapper_lines(text: &Rope, wrapper: &TextWrapper, expected_lines: &[&[&str]]) {
            let mut actual_lines = vec![];
            let mut offset = 0;
            for line in wrapper.lines.iter() {
                actual_lines.push(
                    line.wrapped_lines
                        .iter()
                        .map(|range| text.slice(offset + range.start..offset + range.end))
                        .collect::<Vec<_>>(),
                );
                // +1 \n
                offset += line.len() + 1;
            }
            assert_eq!(actual_lines, expected_lines);
        }

        wrapper._update(&text, &(0..text.len()), &text, &mut fake_wrap_line);
        assert_eq!(wrapper.lines.len(), 4);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["Hello, 世界!\r"],
                &["This is second line."],
                &["This is third line."],
                &["这里是第 4 行。"],
            ],
        );

        // Add a new text to end
        let range = text.len()..text.len();
        let new_text = "New text";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "Hello, 世界!\r\nThis is second line.\nThis is third line.\n这里是第 4 行。New text"
        );
        assert_eq!(wrapper.lines.len(), 4);
        assert_eq!(wrapper.lines.len(), 4);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["Hello, 世界!\r"],
                &["This is second line."],
                &["This is third line."],
                &["这里是第 4 行。New text"],
            ],
        );

        // Replace first line `Hello` to `AAA`
        let range = 0..5;
        let new_text = "AAA";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "AAA, 世界!\r\nThis is second line.\nThis is third line.\n这里是第 4 行。New text"
        );
        assert_eq!(wrapper.lines.len(), 4);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["AAA, 世界!\r"],
                &["This is second line."],
                &["This is third line."],
                &["这里是第 4 行。New text"],
            ],
        );

        // Remove the second line
        let start_offset = text.line_start_offset(1);
        let end_offset = text.line_end_offset(1);
        let range = start_offset..end_offset + 1;
        text.replace(range.clone(), "");
        wrapper._update(&text, &range, &Rope::from(""), &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "AAA, 世界!\r\nThis is third line.\n这里是第 4 行。New text"
        );
        assert_eq!(wrapper.lines.len(), 3);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["AAA, 世界!\r"],
                &["This is third line."],
                &["这里是第 4 行。New text"],
            ],
        );

        // Replace the first 2 lines to "This is a new line."
        let range = text.line_start_offset(0)..text.line_end_offset(1) + 1;
        let new_text = "This is a new line.\nThis is new line 2.\n";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "This is a new line.\nThis is new line 2.\n这里是第 4 行。New text"
        );
        assert_eq!(wrapper.lines.len(), 3);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["This is a new line."],
                &["This is new line 2."],
                &["这里是第 4 行。New text"],
            ],
        );

        // Add a new line at the end
        let range = text.len()..text.len();
        let new_text = "\nThis is a new line at the end.";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "This is a new line.\nThis is new line 2.\n这里是第 4 行。New text\nThis is a new line at the end."
        );
        assert_eq!(wrapper.lines.len(), 4);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["This is a new line."],
                &["This is new line 2."],
                &["这里是第 4 行。New text"],
                &["This is a new line at the end."],
            ],
        );

        // Add a new line at the beginning
        let range = 0..0;
        let new_text = "This is a new line at the beginning.\n";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "This is a new line at the beginning.\nThis is a new line.\nThis is new line 2.\n这里是第 4 行。New text\nThis is a new line at the end."
        );
        assert_eq!(wrapper.lines.len(), 5);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["This is a new line at the beginning."],
                &["This is a new line."],
                &["This is new line 2."],
                &["这里是第 4 行。New text"],
                &["This is a new line at the end."],
            ],
        );

        // Remove all to at least one line in `lines`.
        let range = 0..text.len();
        let new_text = "";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), &mut fake_wrap_line);
        assert_eq!(text.to_string(), "");
        assert_eq!(wrapper.lines.len(), 1);
        assert_eq!(wrapper.lines[0].wrapped_lines, vec![0..0]);

        // Test update_all
        let range = 0..text.len();
        let new_text = "This is a full text.\nThis is a second line.";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &text, &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "This is a full text.\nThis is a second line."
        );
        assert_eq!(wrapper.lines.len(), 2);
    }

    #[test]
    fn test_line_layout() {
        let mut line_layout = LineLayout::new();

        let line1 = ShapedLine::default().with_len(100);
        let line2 = ShapedLine::default().with_len(50);
        let wrapped_lines = smallvec::smallvec![line1, line2];
        line_layout.set_wrapped_lines(wrapped_lines);
        assert_eq!(line_layout.len(), 150);
        assert_eq!(line_layout.wrapped_lines.len(), 2);
    }

    #[test]
    fn test_offset_to_display_point() {
        let font = gpui::Font {
            family: "Arial".into(),
            weight: FontWeight::default(),
            style: FontStyle::Normal,
            features: FontFeatures::default(),
            fallbacks: None,
        };

        let mut wrapper = TextWrapper::new(font, px(14.), None);
        wrapper.text = Rope::from(
            "Hello, 世界!\r\nThis is second line.\nThis is third line.\n这里是第 4 行。",
        );
        wrapper.lines = vec![
            // range: 0..15
            LineItem {
                line: Rope::from("Hello, 世界!\r"),
                wrapped_lines: vec![0..15],
            },
            // range: 16..36
            LineItem {
                line: Rope::from("This is second line."),
                wrapped_lines: vec![0..10, 10..20],
            },
            // range: 37..56
            LineItem {
                line: Rope::from("This is third line."),
                wrapped_lines: vec![0..9, 9..15, 15..20],
            },
            // range: 57..79
            LineItem {
                line: Rope::from("这里是第 4 行。"),
                wrapped_lines: vec![0..22],
            },
        ];

        assert_eq!(
            wrapper.offset_to_display_point(12),
            DisplayPoint::new(0, 0, 12)
        );
        assert_eq!(
            wrapper.offset_to_display_point(15),
            DisplayPoint::new(0, 0, 15)
        );

        assert_eq!(
            wrapper.offset_to_display_point(16),
            DisplayPoint::new(1, 0, 0)
        );
        assert_eq!(
            wrapper.offset_to_display_point(21),
            DisplayPoint::new(1, 0, 5)
        );
        assert_eq!(
            wrapper.offset_to_display_point(27),
            DisplayPoint::new(2, 1, 1)
        );
        assert_eq!(
            wrapper.offset_to_display_point(37),
            DisplayPoint::new(3, 0, 0)
        );
        assert_eq!(
            wrapper.offset_to_display_point(54),
            DisplayPoint::new(5, 2, 2)
        );
        assert_eq!(
            wrapper.offset_to_display_point(59),
            DisplayPoint::new(6, 0, 2)
        );

        assert_eq!(
            wrapper.display_point_to_offset(DisplayPoint::new(6, 0, 2)),
            59
        );
        assert_eq!(
            wrapper.display_point_to_offset(DisplayPoint::new(5, 2, 2)),
            54
        );
        assert_eq!(
            wrapper.display_point_to_offset(DisplayPoint::new(3, 0, 0)),
            37
        );
        assert_eq!(
            wrapper.display_point_to_offset(DisplayPoint::new(2, 1, 1)),
            27
        );
        assert_eq!(
            wrapper.display_point_to_offset(DisplayPoint::new(1, 0, 5)),
            21
        );
        assert_eq!(
            wrapper.display_point_to_offset(DisplayPoint::new(1, 0, 0)),
            16
        );
        assert_eq!(
            wrapper.display_point_to_offset(DisplayPoint::new(0, 0, 15)),
            15
        );
    }
}
