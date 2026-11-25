//! Minimal markdown for GitHub release notes: headings, bullets, **bold**, `code`, and links
//! (`[text](url)` and bare http(s) URLs) that open in the browser. Anything else shows as text.

use eframe::egui;

use crate::theme;

#[derive(Clone, Debug, PartialEq)]
pub enum Span {
    Plain(String),
    Bold(String),
    Code(String),
    Link { text: String, url: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    Blank,
    Heading(Vec<Span>),
    Bullet(Vec<Span>),
    Text(Vec<Span>),
}

pub fn parse(md: &str) -> Vec<Line> {
    md.replace("\r\n", "\n")
        .lines()
        .map(|raw| {
            let l = raw.trim_end();
            let t = l.trim_start();
            if t.is_empty() {
                Line::Blank
            } else if t.starts_with('#') {
                Line::Heading(spans(t.trim_start_matches('#').trim()))
            } else if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
                Line::Bullet(spans(rest))
            } else {
                Line::Text(spans(t))
            }
        })
        .collect()
}

fn is_url_char(c: char) -> bool {
    !c.is_whitespace() && !matches!(c, '<' | '>' | '"')
}

/// Split inline markdown into spans.
pub fn spans(s: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut rest = s;
    let flush = |plain: &mut String, out: &mut Vec<Span>| {
        if !plain.is_empty() {
            out.push(Span::Plain(std::mem::take(plain)));
        }
    };
    while let Some(c) = rest.chars().next() {
        if let Some(r) = rest.strip_prefix("**") {
            if let Some(end) = r.find("**").filter(|&e| e > 0) {
                flush(&mut plain, &mut out);
                out.push(Span::Bold(r[..end].to_string()));
                rest = &r[end + 2..];
                continue;
            }
        } else if let Some(r) = rest.strip_prefix('`') {
            if let Some(end) = r.find('`').filter(|&e| e > 0) {
                flush(&mut plain, &mut out);
                out.push(Span::Code(r[..end].to_string()));
                rest = &r[end + 1..];
                continue;
            }
        } else if let Some(r) = rest.strip_prefix('[') {
            if let Some((text, after)) = r.split_once("](") {
                if let Some((url, tail)) = after.split_once(')') {
                    if !text.contains('[') && url.starts_with("http") {
                        flush(&mut plain, &mut out);
                        out.push(Span::Link { text: text.to_string(), url: url.to_string() });
                        rest = tail;
                        continue;
                    }
                }
            }
        } else if rest.starts_with("http://") || rest.starts_with("https://") {
            let len = rest.find(|c: char| !is_url_char(c)).unwrap_or(rest.len());
            // Trailing punctuation belongs to the sentence, not the link.
            let url = rest[..len].trim_end_matches(['.', ',', ';', ':', '!', '?', ')']);
            flush(&mut plain, &mut out);
            out.push(Span::Link { text: url.to_string(), url: url.to_string() });
            rest = &rest[url.len()..];
            continue;
        }
        plain.push(c);
        rest = &rest[c.len_utf8()..];
    }
    flush(&mut plain, &mut out);
    out
}

fn show_spans(ui: &mut egui::Ui, spans: &[Span], size: f32, strong: bool) {
    // Spans are slices of one sentence: the spaces are already in the text.
    ui.spacing_mut().item_spacing.x = 0.0;
    for sp in spans {
        match sp {
            Span::Plain(t) => {
                let mut r = egui::RichText::new(t).size(size);
                if strong {
                    r = r.strong();
                }
                ui.label(r);
            }
            Span::Bold(t) => {
                ui.label(egui::RichText::new(t).size(size).strong());
            }
            Span::Code(t) => {
                ui.label(egui::RichText::new(t).size(size).monospace().color(theme::DIM));
            }
            Span::Link { text, url } => {
                ui.hyperlink_to(egui::RichText::new(text).size(size), url);
            }
        }
    }
}

/// Draw release notes; links are clickable (egui opens them in the system browser).
pub fn show(ui: &mut egui::Ui, md: &str) {
    for line in parse(md) {
        match line {
            Line::Blank => ui.add_space(6.0),
            Line::Heading(s) => {
                ui.horizontal_wrapped(|ui| show_spans(ui, &s, 15.0, true));
            }
            Line::Bullet(s) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label("•  ");
                    show_spans(ui, &s, 13.0, false);
                });
            }
            Line::Text(s) => {
                ui.horizontal_wrapped(|ui| show_spans(ui, &s, 13.0, false));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(text: &str, url: &str) -> Span {
        Span::Link { text: text.into(), url: url.into() }
    }

    #[test]
    fn the_real_release_note_is_bold_label_plus_a_bare_link() {
        let s = spans("**Full Changelog**: https://github.com/ItsSebis/MacMischer/compare/v0.1.3...v0.1.4");
        assert_eq!(
            s,
            vec![
                Span::Bold("Full Changelog".into()),
                Span::Plain(": ".into()),
                link("https://github.com/ItsSebis/MacMischer/compare/v0.1.3...v0.1.4", "https://github.com/ItsSebis/MacMischer/compare/v0.1.3...v0.1.4"),
            ]
        );
    }

    #[test]
    fn markdown_links_and_trailing_punctuation() {
        assert_eq!(
            spans("see [the PR](https://x.io/pr/1), or https://x.io/a."),
            vec![
                Span::Plain("see ".into()),
                link("the PR", "https://x.io/pr/1"),
                Span::Plain(", or ".into()),
                link("https://x.io/a", "https://x.io/a"),
                Span::Plain(".".into()),
            ]
        );
    }

    #[test]
    fn unclosed_markers_stay_text() {
        assert_eq!(spans("a **b and `c"), vec![Span::Plain("a **b and `c".into())]);
        assert_eq!(spans("[x](ftp://no)"), vec![Span::Plain("[x](ftp://no)".into())]);
    }

    #[test]
    fn line_kinds() {
        let l = parse("## New\n\n- one\n* two\r\nplain");
        assert!(matches!(l[0], Line::Heading(_)));
        assert_eq!(l[1], Line::Blank);
        assert!(matches!(l[2], Line::Bullet(_)));
        assert!(matches!(l[3], Line::Bullet(_)));
        assert!(matches!(l[4], Line::Text(_)));
    }
}
