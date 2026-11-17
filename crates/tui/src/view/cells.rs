//! Terminal cell arithmetic. Byte offsets index a `str`, graphemes are what
//! a reader sees as one character (a base letter with its combining marks,
//! or a ZWJ emoji sequence), and cells are the terminal columns a grapheme
//! occupies: two for CJK and most emoji. Wrapping and truncation measure
//! cells and cut only between graphemes. Widths come from `unicode-width`;
//! a terminal that draws a ZWJ sequence or a flag as several glyphs shows it
//! wider than counted here, and nothing below can detect that.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Columns a tab advances to, counted from the start of the line.
const TAB_STOP: usize = 4;

/// Terminal cells `text` occupies on one line.
pub(crate) fn cells(text: &str) -> usize {
    text.graphemes(true).map(UnicodeWidthStr::width).sum()
}

/// The longest prefix of `text` that fits in `max` cells, with its width.
pub(crate) fn take_cells(text: &str, max: usize) -> (String, usize) {
    let mut taken = String::new();
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        let width = grapheme.width();
        if used + width > max {
            break;
        }
        taken.push_str(grapheme);
        used += width;
    }
    (taken, used)
}

/// `text` cut to `max` cells, ending with `…` when anything was cut.
pub(crate) fn truncate_cells(text: &str, max: usize) -> String {
    if cells(text) <= max {
        return text.to_string();
    }
    let (mut taken, _) = take_cells(text, max.saturating_sub(1));
    if max > 0 {
        taken.push('…');
    }
    taken
}

/// Splits `text` into rows of at most `width` cells. A grapheme wider than
/// `width` gets a row of its own rather than being dropped.
pub(crate) fn split_cells(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut current = String::new();
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        let grapheme_width = grapheme.width();
        if used + grapheme_width > width && !current.is_empty() {
            rows.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push_str(grapheme);
        used += grapheme_width;
    }
    if !current.is_empty() || rows.is_empty() {
        rows.push(current);
    }
    rows
}

/// Word-wraps one paragraph: the first row holds `first_width` cells, the
/// rest `next_width`. Runs of whitespace collapse to one space; a word
/// longer than a row, such as unspaced Japanese text, is split between
/// graphemes.
pub(crate) fn wrap_words(paragraph: &str, first_width: usize, next_width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut current = String::new();
    let mut used = 0;
    let mut width = first_width.max(1);
    for word in paragraph.split_whitespace() {
        let word_width = cells(word);
        if !current.is_empty() && used + 1 + word_width <= width {
            current.push(' ');
            current.push_str(word);
            used += 1 + word_width;
            continue;
        }
        if !current.is_empty() {
            rows.push(std::mem::take(&mut current));
            width = next_width.max(1);
        }
        let mut pieces = split_cells(word, width);
        let last = pieces.pop().unwrap_or_default();
        for piece in pieces {
            rows.push(piece);
            width = next_width.max(1);
        }
        used = cells(&last);
        current = last;
    }
    if !current.is_empty() {
        rows.push(current);
    }
    rows
}

/// Wraps text whose spacing matters, such as a diff or code: every
/// character is kept, tabs become spaces, and each line is split at
/// `width` cells.
pub(crate) fn wrap_preformatted(text: &str, width: usize) -> Vec<String> {
    text.lines()
        .flat_map(|line| split_cells(&expand_tabs(line.trim_end_matches('\r')), width))
        .collect()
}

fn expand_tabs(line: &str) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut out = String::new();
    let mut column = 0;
    for grapheme in line.graphemes(true) {
        if grapheme == "\t" {
            let spaces = TAB_STOP - column % TAB_STOP;
            out.push_str(&" ".repeat(spaces));
            column += spaces;
        } else {
            out.push_str(grapheme);
            column += grapheme.width();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAMILY: &str = "👨\u{200d}👩\u{200d}👧";
    const COMBINING: &str = "e\u{301}";

    #[test]
    fn bytes_graphemes_and_cells_are_counted_apart() {
        let text = format!("日本{COMBINING}{FAMILY}");
        assert_eq!(text.len(), 6 + 3 + 18);
        assert_eq!(text.graphemes(true).count(), 4);
        assert_eq!(cells(&text), 2 + 2 + 1 + 2);
    }

    #[test]
    fn splitting_never_cuts_a_grapheme_or_drops_content() {
        let text = format!("あい{COMBINING}{FAMILY}う");
        let rows = split_cells(&text, 3);

        assert_eq!(rows.concat(), text);
        assert_eq!(
            rows,
            ["あ", format!("い{COMBINING}").as_str(), FAMILY, "う"]
        );
        assert!(rows.iter().all(|row| cells(row) <= 3));
        // A row narrower than one wide grapheme still shows it.
        assert_eq!(split_cells("漢字", 1), ["漢", "字"]);
    }

    #[test]
    fn word_wrap_measures_cells_and_splits_unspaced_japanese() {
        let rows = wrap_words("設定ファイルを読み込みます。 done", 10, 10);

        assert_eq!(rows, ["設定ファイ", "ルを読み込", "みます。", "done"]);
        assert!(rows.iter().all(|row| cells(row) <= 10));
    }

    #[test]
    fn truncation_counts_cells_and_marks_the_cut() {
        assert_eq!(truncate_cells("日本語テキスト", 7), "日本語…");
        assert_eq!(truncate_cells("日本語", 6), "日本語");
        assert_eq!(truncate_cells(&format!("ab{FAMILY}cd"), 4), "ab…");
        assert!(cells(&truncate_cells("全角全角全角", 5)) <= 5);
    }

    #[test]
    fn preformatted_text_keeps_indentation_and_expands_tabs() {
        let rows = wrap_preformatted("+    indented\r\n-\tx\n\n 全角全角", 6);
        assert_eq!(
            rows,
            ["+    i", "ndente", "d", "-   x", "", " 全角", "全角"]
        );
    }
}
