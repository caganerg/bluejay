//! A small pulldown-cmark -> egui renderer.
//!
//! It covers the everyday markdown subset (headings, emphasis, lists, quotes,
//! code, links, rules, tables) plus `[[wiki links]]`, which are detected in the
//! text stream so that links inside code spans and fences are left alone.

use eframe::egui::{
    self, Color32, Rect, RichText, Shape, Stroke, TextStyle, TextWrapMode, Ui, WidgetText, pos2,
};
use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};

/// Something the reader clicked in the preview.
pub enum Action {
    /// A `[[wiki link]]`: open the note with this name.
    OpenNote(String),
}

const TEXT: Color32 = Color32::from_rgb(0xdc, 0xdd, 0xde);
const MUTED: Color32 = Color32::from_rgb(0x9a, 0x9d, 0xa2);
const LINK: Color32 = Color32::from_rgb(0x7f, 0xa8, 0xf5);
const CODE_FG: Color32 = Color32::from_rgb(0xe6, 0x9a, 0x9a);
const CODE_BG: Color32 = Color32::from_rgb(0x25, 0x27, 0x2b);
const HEAD_BG: Color32 = Color32::from_rgb(0x2b, 0x2e, 0x33);
const RULE: Color32 = Color32::from_rgb(0x44, 0x47, 0x4d);
const BODY_SIZE: f32 = 15.0;
/// Horizontal gap between two table columns.
const CELL_GAP: f32 = 18.0;
/// Vertical padding between the header text and the band drawn behind it.
const HEAD_PAD: f32 = 3.0;

#[derive(Clone, Copy, Default, PartialEq)]
struct Style {
    strong: bool,
    em: bool,
    strike: bool,
    heading: Option<u8>,
    quote: bool,
}

enum Inline {
    Text(String, Style),
    Code(String),
    Link { text: String, url: String },
    Wiki(String),
    Break,
}

fn font_size(style: &Style) -> f32 {
    match style.heading {
        Some(1) => 27.0,
        Some(2) => 22.0,
        Some(3) => 19.0,
        Some(4) => 17.0,
        Some(5) => 15.5,
        Some(6) => 14.5,
        _ => BODY_SIZE,
    }
}

fn rich(text: &str, style: &Style) -> RichText {
    let mut t = RichText::new(text)
        .size(font_size(style))
        .color(if style.quote { MUTED } else { TEXT });
    if style.strong || style.heading.is_some() {
        t = t.strong();
    }
    if style.em {
        t = t.italics();
    }
    if style.strike {
        t = t.strikethrough();
    }
    t
}

fn code_rich(code: &str) -> RichText {
    RichText::new(code)
        .monospace()
        .size(BODY_SIZE - 1.0)
        .color(CODE_FG)
        .background_color(CODE_BG)
}

fn link_rich(text: &str) -> RichText {
    RichText::new(text).size(BODY_SIZE).color(LINK)
}

/// The parser extensions the renderer knows how to draw.
fn options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_TABLES);
    options
}

/// Render `source` into `ui`, returning a click on a wiki link if there was one.
pub fn render(ui: &mut Ui, source: &str) -> Option<Action> {
    let mut action = None;
    let mut inlines: Vec<Inline> = Vec::new();
    let mut style = Style::default();

    // Consecutive `Event::Text` are merged before scanning for `[[..]]`, because
    // the parser splits unmatched brackets across several text events.
    let mut text_buf = String::new();
    let mut text_style = Style::default();

    let mut list_stack: Vec<Option<u64>> = Vec::new();
    let mut prefix: Option<String> = None;
    let mut quote_depth = 0usize;
    let mut link: Option<(String, String)> = None; // (url, accumulated text)
    let mut code_block: Option<String> = None;
    let mut table: Option<Table> = None;

    macro_rules! flush_text {
        () => {
            if !text_buf.is_empty() {
                push_text(&mut inlines, &text_buf, text_style);
                text_buf.clear();
            }
        };
    }

    for event in Parser::new_ext(source, options()) {
        // Text is the only event we buffer; everything else ends a run.
        if !matches!(event, Event::Text(_)) {
            flush_text!();
        }

        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => style.heading = Some(level as u8),
                Tag::BlockQuote(_) => {
                    quote_depth += 1;
                    style.quote = true;
                }
                Tag::CodeBlock(_) => code_block = Some(String::new()),
                Tag::List(start) => list_stack.push(start),
                Tag::Item => {
                    prefix = Some(match list_stack.last_mut() {
                        Some(Some(n)) => {
                            let marker = format!("{n}. ");
                            *n += 1;
                            marker
                        }
                        _ => "• ".to_owned(),
                    });
                }
                Tag::Emphasis => style.em = true,
                Tag::Strong => style.strong = true,
                Tag::Strikethrough => style.strike = true,
                Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                    link = Some((dest_url.to_string(), String::new()));
                }
                Tag::Table(aligns) => table = Some(Table { aligns, ..Table::default() }),
                _ => {}
            },

            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    flush_block(ui, &mut inlines, prefix.take(), indent(&list_stack, quote_depth), &mut action);
                    ui.add_space(4.0);
                }
                TagEnd::Heading(_) => {
                    ui.add_space(6.0);
                    flush_block(ui, &mut inlines, prefix.take(), indent(&list_stack, quote_depth), &mut action);
                    style.heading = None;
                    ui.add_space(4.0);
                }
                TagEnd::CodeBlock => {
                    if let Some(code) = code_block.take() {
                        code_block_ui(ui, code.trim_end(), indent(&list_stack, quote_depth));
                    }
                }
                TagEnd::BlockQuote(_) => {
                    quote_depth = quote_depth.saturating_sub(1);
                    style.quote = quote_depth > 0;
                }
                TagEnd::List(_) => {
                    list_stack.pop();
                    if list_stack.is_empty() {
                        ui.add_space(4.0);
                    }
                }
                TagEnd::Item => {
                    // Tight lists have no inner paragraph, so flush here too.
                    if !inlines.is_empty() || prefix.is_some() {
                        flush_block(ui, &mut inlines, prefix.take(), indent(&list_stack, quote_depth), &mut action);
                    }
                }
                TagEnd::Emphasis => style.em = false,
                TagEnd::Strong => style.strong = false,
                TagEnd::Strikethrough => style.strike = false,
                TagEnd::Link | TagEnd::Image => {
                    if let Some((url, text)) = link.take() {
                        let text = if text.is_empty() { url.clone() } else { text };
                        inlines.push(Inline::Link { text, url });
                    }
                }
                TagEnd::TableCell => {
                    if let Some(table) = table.as_mut() {
                        table.row.push(std::mem::take(&mut inlines));
                    }
                }
                TagEnd::TableHead => {
                    if let Some(table) = table.as_mut() {
                        table.head = std::mem::take(&mut table.row);
                        // Markdown gives header cells no emphasis of their own.
                        for inline in table.head.iter_mut().flatten() {
                            if let Inline::Text(_, style) = inline {
                                style.strong = true;
                            }
                        }
                    }
                }
                TagEnd::TableRow => {
                    if let Some(table) = table.as_mut() {
                        let row = std::mem::take(&mut table.row);
                        table.body.push(row);
                    }
                }
                TagEnd::Table => {
                    if let Some(table) = table.take() {
                        table_ui(ui, &table, indent(&list_stack, quote_depth), &mut action);
                        ui.add_space(4.0);
                    }
                }
                _ => {}
            },

            Event::Text(text) => {
                if let Some(code) = code_block.as_mut() {
                    code.push_str(&text);
                } else if let Some((_, buf)) = link.as_mut() {
                    buf.push_str(&text);
                } else {
                    if text_style != style {
                        flush_text!();
                        text_style = style;
                    }
                    text_buf.push_str(&text);
                }
            }
            Event::Code(code) => {
                if let Some((_, buf)) = link.as_mut() {
                    buf.push_str(&code);
                } else {
                    inlines.push(Inline::Code(code.to_string()));
                }
            }
            Event::SoftBreak => {
                // A soft break is just a space in rendered markdown.
                if let Some((_, buf)) = link.as_mut() {
                    buf.push(' ');
                } else {
                    text_style = style;
                    text_buf.push(' ');
                }
            }
            Event::HardBreak => inlines.push(Inline::Break),
            Event::Rule => {
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);
            }
            Event::TaskListMarker(done) => {
                let mark = if done { "[x] " } else { "[ ] " };
                inlines.push(Inline::Text(mark.to_owned(), style));
            }
            _ => {}
        }
    }

    flush_text!();
    if !inlines.is_empty() {
        flush_block(ui, &mut inlines, prefix.take(), indent(&list_stack, quote_depth), &mut action);
    }

    action
}

fn indent(list_stack: &[Option<u64>], quote_depth: usize) -> f32 {
    list_stack.len().saturating_sub(1) as f32 * 20.0 + quote_depth as f32 * 14.0
}

/// Split a text run into wiki links and everything else.
fn push_text(inlines: &mut Vec<Inline>, source: &str, style: Style) {
    let mut rest = source;
    while let Some(open) = rest.find("[[") {
        let after = &rest[open + 2..];
        match after.find("]]") {
            Some(close) if close > 0 && !after[..close].contains('[') => {
                if open > 0 {
                    inlines.push(Inline::Text(rest[..open].to_owned(), style));
                }
                inlines.push(Inline::Wiki(after[..close].trim().to_owned()));
                rest = &after[close + 2..];
            }
            _ => {
                inlines.push(Inline::Text(rest[..open + 2].to_owned(), style));
                rest = after;
            }
        }
    }
    if !rest.is_empty() {
        inlines.push(Inline::Text(rest.to_owned(), style));
    }
}

/// Lay out one block of inline content as a wrapping row of small widgets.
fn flush_block(
    ui: &mut Ui,
    inlines: &mut Vec<Inline>,
    prefix: Option<String>,
    indent: f32,
    action: &mut Option<Action>,
) {
    if inlines.is_empty() && prefix.is_none() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if indent > 0.0 {
            ui.add_space(indent);
        }
        if let Some(prefix) = prefix {
            ui.label(RichText::new(prefix).size(BODY_SIZE).color(MUTED));
        }
        draw_inlines(ui, inlines, true, action);
    });
    inlines.clear();
}

/// Draw a run of inline content into the row the caller has already opened.
///
/// `split_words` gives every word its own label so a wrapping layout can break
/// between them. Table cells don't wrap and are measured as whole strings, so
/// they pass `false` to keep the drawn width equal to the measured one.
fn draw_inlines(ui: &mut Ui, inlines: &[Inline], split_words: bool, action: &mut Option<Action>) {
    for inline in inlines {
        match inline {
            Inline::Text(text, style) => {
                if split_words {
                    for word in text.split_inclusive(' ') {
                        ui.label(rich(word, style));
                    }
                } else {
                    ui.label(rich(text, style));
                }
            }
            Inline::Code(code) => {
                ui.label(code_rich(code));
            }
            Inline::Link { text, url } => {
                ui.hyperlink_to(link_rich(text), url);
            }
            Inline::Wiki(name) => {
                if ui.link(link_rich(&format!("[[{name}]]"))).clicked() {
                    *action = Some(Action::OpenNote(name.clone()));
                }
            }
            Inline::Break => ui.end_row(),
        }
    }
}

/// A table buffered while it is parsed.
///
/// It cannot be drawn as it streams in: a column is only as wide as its widest
/// cell, and that isn't known until the last row has been read.
#[derive(Default)]
struct Table {
    aligns: Vec<Alignment>,
    head: Vec<Vec<Inline>>,
    body: Vec<Vec<Vec<Inline>>>,
    /// Cells of the row currently being parsed.
    row: Vec<Vec<Inline>>,
}

/// Width one inline will take up, asking for the same galley the label will use.
fn inline_width(ui: &Ui, inline: &Inline) -> f32 {
    let text: WidgetText = match inline {
        Inline::Text(text, style) => rich(text, style).into(),
        Inline::Code(code) => code_rich(code).into(),
        Inline::Link { text, .. } => link_rich(text).into(),
        Inline::Wiki(name) => link_rich(&format!("[[{name}]]")).into(),
        Inline::Break => return 0.0,
    };
    text.into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, TextStyle::Body)
        .size()
        .x
}

/// Cells are drawn with zero item spacing, so their width is just the sum.
fn cell_width(ui: &Ui, cell: &[Inline]) -> f32 {
    cell.iter().map(|inline| inline_width(ui, inline)).sum()
}

/// Draw a table on a grid measured up front.
///
/// egui's `Grid` only learns its column widths from the previous frame, which
/// would put the alignment padding in the wrong place on the frame a note is
/// opened, so the cells are measured here and padded by hand instead.
fn table_ui(ui: &mut Ui, table: &Table, indent: f32, action: &mut Option<Action>) {
    let cols = table
        .aligns
        .len()
        .max(table.head.len())
        .max(table.body.iter().map(Vec::len).max().unwrap_or(0));
    if cols == 0 {
        return;
    }

    let measured = |row: &[Vec<Inline>]| -> Vec<f32> {
        (0..cols)
            .map(|col| row.get(col).map_or(0.0, |cell| cell_width(ui, cell)))
            .collect()
    };
    let head_widths = measured(&table.head);
    let body_widths: Vec<Vec<f32>> = table.body.iter().map(|row| measured(row)).collect();

    let mut widths = vec![0.0f32; cols];
    for row in std::iter::once(&head_widths).chain(&body_widths) {
        for (col, width) in row.iter().enumerate() {
            widths[col] = widths[col].max(*width);
        }
    }
    let total = widths.iter().sum::<f32>() + CELL_GAP * (cols - 1) as f32;

    // Reserved now and filled in once the header row's height is known, so the
    // band ends up behind the text rather than on top of it.
    let band = ui.painter().add(Shape::Noop);
    let left = ui.cursor().left() + indent;

    ui.add_space(HEAD_PAD);
    let head_rect = table_row(ui, &table.head, &head_widths, &widths, &table.aligns, indent, action);
    ui.add_space(HEAD_PAD);

    let band_rect = Rect::from_min_max(
        pos2(left - 4.0, head_rect.top() - HEAD_PAD),
        pos2(left + total + 4.0, head_rect.bottom() + HEAD_PAD),
    );
    ui.painter()
        .set(band, Shape::rect_filled(band_rect, 2.0, HEAD_BG));
    ui.painter()
        .hline(band_rect.x_range(), band_rect.bottom(), Stroke::new(1.0, RULE));

    for (row, row_widths) in table.body.iter().zip(&body_widths) {
        table_row(ui, row, row_widths, &widths, &table.aligns, indent, action);
    }
}

/// One row, each cell padded from its own width out to its column's width.
fn table_row(
    ui: &mut Ui,
    cells: &[Vec<Inline>],
    cell_widths: &[f32],
    widths: &[f32],
    aligns: &[Alignment],
    indent: f32,
    action: &mut Option<Action>,
) -> Rect {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if indent > 0.0 {
            ui.add_space(indent);
        }
        for (col, width) in widths.iter().enumerate() {
            if col > 0 {
                ui.add_space(CELL_GAP);
            }
            let slack = (width - cell_widths.get(col).copied().unwrap_or(0.0)).max(0.0);
            let (before, after) = match aligns.get(col) {
                Some(Alignment::Center) => (slack / 2.0, slack / 2.0),
                Some(Alignment::Right) => (slack, 0.0),
                _ => (0.0, slack),
            };
            ui.add_space(before);
            if let Some(cell) = cells.get(col) {
                draw_inlines(ui, cell, false, action);
            }
            ui.add_space(after);
        }
    })
    .response
    .rect
}

fn code_block_ui(ui: &mut Ui, code: &str, indent: f32) {
    ui.horizontal(|ui| {
        if indent > 0.0 {
            ui.add_space(indent);
        }
        egui::Frame::group(ui.style())
            .fill(CODE_BG)
            .stroke(egui::Stroke::NONE)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(code)
                        .monospace()
                        .size(BODY_SIZE - 1.0)
                        .color(CODE_FG),
                );
            });
    });
    ui.add_space(4.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(source: &str) -> Vec<String> {
        let mut inlines = Vec::new();
        push_text(&mut inlines, source, Style::default());
        inlines
            .iter()
            .map(|inline| match inline {
                Inline::Text(text, _) => format!("text:{text}"),
                Inline::Wiki(name) => format!("wiki:{name}"),
                _ => "other".to_owned(),
            })
            .collect()
    }

    #[test]
    fn finds_wiki_links() {
        assert_eq!(split("see [[Other Note]] now"), ["text:see ", "wiki:Other Note", "text: now"]);
        assert_eq!(split("[[A]][[B]]"), ["wiki:A", "wiki:B"]);
        assert_eq!(split("[[ Spaced ]]"), ["wiki:Spaced"]);
    }

    #[test]
    fn leaves_other_brackets_alone() {
        assert_eq!(split("[[unclosed"), ["text:[[", "text:unclosed"]);
        assert_eq!(split("[[]]"), ["text:[[", "text:]]"]);
        assert_eq!(split("plain [text] here"), ["text:plain [text] here"]);
    }

    /// Exercises the whole renderer to catch panics in the layout code.
    #[test]
    fn renders_a_document_without_panicking() {
        let source = "\
# Title

Some **bold**, *italic*, ~~struck~~ text with `code`, a [link](https://example.com)
and a [[Wiki Link]] across a soft break.

- one
- two
  1. nested
  2. more

> a quote with [[Another Note]]

```rust
fn main() { let x = \"[[not a link]]\"; }
```

---

| not | a table |
";
        eframe::egui::__run_test_ui(|ui| {
            render(ui, source);
        });
    }

    /// Tables are the one block that is buffered whole, so they get a parse
    /// check on top of the layout pass.
    #[test]
    fn renders_a_table() {
        let source = "\
| Name | Count | Note |
|:-----|:-----:|-----:|
| alpha | 1 | see [[Other]] |
| beta | 22 | `code` |
";
        let mut aligns = Vec::new();
        let (mut heads, mut rows, mut cells) = (0, 0, 0);
        for event in Parser::new_ext(source, options()) {
            match event {
                Event::Start(Tag::Table(a)) => aligns = a,
                Event::Start(Tag::TableHead) => heads += 1,
                Event::Start(Tag::TableRow) => rows += 1,
                Event::Start(Tag::TableCell) => cells += 1,
                _ => {}
            }
        }
        assert_eq!(aligns, [Alignment::Left, Alignment::Center, Alignment::Right]);
        assert_eq!((heads, rows, cells), (1, 2, 9));

        eframe::egui::__run_test_ui(|ui| {
            render(ui, source);
        });
    }
}
