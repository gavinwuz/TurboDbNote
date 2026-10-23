# GPUI Component 0.5.1 adaptation

Source: crates.io gpui-component 0.5.1, https://github.com/longbridge/gpui-component, Apache-2.0 (see LICENSE-APACHE).

The vendored source preserves version 0.5.1 and the paired GPUI/assets dependencies. The input adaptation adds InputState::scroll_beyond_last_line(bool), default true, and gates code-editor trailing scroll space with it. TurboDBNote Note editors opt out. InputState::content_height() exposes existing measured visual-line geometry so soft wrapping sizes editors correctly. Registry sources are never modified. Recheck these two input changes together when upgrading the paired dependencies.

InputState::selection_byte_range() exposes the existing UTF-8 selection for statement-range execution without reading or changing the clipboard.

InputState::deferred_highlighting(light_language) opts an editor into one coalescing background parse/capture worker. Word boundaries schedule after the next frame (16ms), other edits after 150ms idle, IME marking suspends work. Source revisions and theme snapshots guard cached styles. A bounded edit map keeps unaffected styles aligned while inserted/replaced bytes temporarily use the default style; stale byte ranges are never painted over new text. The default component behavior remains synchronous for other inputs. Full-tree replacement edits allow skipped revisions, and very long physical lines use the registered lightweight language. Input selection, clipboard, undo and text content are unchanged.

Deferred editors also opt into incremental soft-wrap reflow: unchanged prefix segments of the edited physical line are retained and the preceding soft line plus changed suffix are rewrapped. Other component inputs retain the original full-line behavior; font/wrap-width changes still rebuild the layout.

Input-first editors retain at most 512 shaped fragments / 512 KiB of text, keyed by fragment text, font runs/size; changing wrap width changes fragment boundaries. They shape only visible soft rows plus caret/selection endpoints, retaining byte lengths and row heights for unshaped rows. Rc-backed per-line geometry and prefix indices avoid cloning shaped glyph vectors or scanning every soft row for caret lookup. Selection and copy use the original full source. Completion providers may consume a selected action item before text insertion, used for explicit cache-miss column loading.

Input painting no longer unconditionally notifies another frame. Geometry/scroll changes still notify, as do existing value, selection, focus and blink transitions. Wrapped-text clipping uses the actual input viewport rather than the scrolled content origin.

Cursor blink pauses coalesce into a single idle timer. Empty completion clearing and unchanged scroll do not notify. Pure highlight-map / geometry helpers are compiled into desktop tests because this vendored package is not a workspace member; text-wrapper internal tests also document Unicode and row-boundary geometry.
