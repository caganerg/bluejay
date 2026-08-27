//! A small pulldown-cmark -> egui renderer.
//!
//! It covers the everyday markdown subset (headings, emphasis, lists, quotes,
//! code, links, rules) plus `[[wiki links]]`, which are detected in the text
//! stream so that links inside code spans and fences are left alone.

use eframe::egui::{self, Color32, RichText, Ui};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

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
const BODY_SIZE: f32 = 15.0;

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

fn rich(text: &str, style: &Style) -> RichText {
    let size = match style.heading {
        Some(1) => 27.0,
        Some(2) => 22.0,
        Some(3) => 19.0,
        Some(4) => 17.0,
        Some(5) => 15.5,
        Some(6) => 14.5,
        _ => BODY_SIZE,
    };
    let mut t = RichText::new(text)
        .size(size)
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

/// Render `source` into `ui`, returning a click on a wiki link if there was one.
pub fn render(ui: &mut Ui, source: &str) -> Option<Action> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

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

    macro_rules! flush_text {
        () => {
            if !text_buf.is_empty() {
                push_text(&mut inlines, &text_buf, text_style);
                text_buf.clear();
            }
        };
    }

    for event in Parser::new_ext(source, options) {
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
        for inline in inlines.iter() {
            match inline {
                // One label per word so the row wraps at word boundaries.
                Inline::Text(text, style) => {
                    for word in text.split_inclusive(' ') {
                        ui.label(rich(word, style));
                    }
                }
                Inline::Code(code) => {
                    ui.label(
                        RichText::new(code)
                            .monospace()
                            .size(BODY_SIZE - 1.0)
                            .color(CODE_FG)
                            .background_color(CODE_BG),
                    );
                }
                Inline::Link { text, url } => {
                    ui.hyperlink_to(RichText::new(text).size(BODY_SIZE).color(LINK), url);
                }
                Inline::Wiki(name) => {
                    let label = RichText::new(format!("[[{name}]]"))
                        .size(BODY_SIZE)
                        .color(LINK);
                    if ui.link(label).clicked() {
                        *action = Some(Action::OpenNote(name.clone()));
                    }
                }
                Inline::Break => ui.end_row(),
            }
        }
    });
    inlines.clear();
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
}
