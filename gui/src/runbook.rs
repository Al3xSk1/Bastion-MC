//! The emergency runbook, taken apart for display.
//!
//! The text is `docs/SCRAM_RUNBOOK.md` as compiled into the program
//! ([`bastion_mc_proto::runbook::SCRAM`]): Markdown, wrapped at a fixed column. Shown as it
//! is in a window narrower than that column, every line breaks twice. [`blocks`] turns it
//! into paragraphs, list items, addresses and commands that the view can lay out at any
//! width.
//!
//! This is not a Markdown implementation. It knows the handful of constructs that one file
//! uses, and the tests hold it to what matters here: every word of the text reaches the
//! screen, once and in order, and the addresses it sets apart are exactly the ones on the
//! allowlist. It only ever sees text that is part of the program — nothing from the
//! launcher, the game or a file is given to it.

use std::sync::LazyLock;

use bastion_mc_proto::runbook;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// `# …`: what the text is.
    Title(String),
    /// `## …`: one of its parts.
    Heading(String),
    /// Prose. `marker` is the list marker it began with (`1.`, `-`) if it is a list item.
    /// `level` says how far in it stands: 0 outside lists, 1 for an item and for what
    /// continues it, 2 for an item of a list inside an item.
    Text { level: u8, marker: Option<String>, text: String },
    /// An address on a line of its own. It is shown, never made into a control: the
    /// controls that open addresses are built from [`runbook::LINKS`], not from this text.
    Link { level: u8, url: String },
    /// An indented command.
    Code(String),
    /// `---`
    Rule,
}

/// The runbook as shipped, taken apart once.
pub static SCRAM: LazyLock<Vec<Block>> = LazyLock::new(|| blocks(runbook::SCRAM));

/// `1. text` or `- text`: the marker and the text.
fn item(line: &str) -> Option<(&str, &str)> {
    if let Some(rest) = line.strip_prefix("- ") {
        return Some(("-", rest));
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    let rest = line[digits..].strip_prefix(". ")?;
    (1..=3).contains(&digits).then(|| (&line[..digits + 1], rest))
}

/// Emphasis marks are not shown: the view has one weight of type.
fn plain(text: &str) -> String {
    text.replace('*', "")
}

pub fn blocks(source: &str) -> Vec<Block> {
    let mut out = Vec::new();
    // The paragraph being collected: its level, its marker, its words so far.
    let mut open: Option<(u8, Option<String>, String)> = None;
    let mut in_list = false;
    fn close(open: &mut Option<(u8, Option<String>, String)>, out: &mut Vec<Block>) {
        if let Some((level, marker, text)) = open.take() {
            out.push(Block::Text { level, marker, text });
        }
    }
    for line in source.lines() {
        let indent = line.len() - line.trim_start().len();
        let body = line.trim();
        if body.is_empty() {
            close(&mut open, &mut out);
            continue;
        }
        if indent == 0 {
            let structural = if let Some(title) = body.strip_prefix("## ") {
                Some(Block::Heading(plain(title)))
            } else if let Some(title) = body.strip_prefix("# ") {
                Some(Block::Title(plain(title)))
            } else if body == "---" {
                Some(Block::Rule)
            } else {
                None
            };
            if let Some(block) = structural {
                close(&mut open, &mut out);
                in_list = false;
                out.push(block);
                continue;
            }
        }
        if let Some((marker, rest)) = item(body) {
            close(&mut open, &mut out);
            in_list = true;
            open = Some((if indent == 0 { 1 } else { 2 }, Some(marker.to_string()), plain(rest)));
            continue;
        }
        // Neither a heading nor the start of an item: how far in does it stand?
        let level = match indent {
            0 => 0,
            1..=4 if in_list => 1,
            _ if in_list => 2,
            // Indented outside a list: a command.
            1..=3 => 0,
            _ => {
                close(&mut open, &mut out);
                out.push(Block::Code(body.to_string()));
                continue;
            }
        };
        if body.starts_with("https://") && !body.contains(char::is_whitespace) {
            close(&mut open, &mut out);
            in_list = in_list && level > 0;
            out.push(Block::Link { level, url: body.to_string() });
            continue;
        }
        match &mut open {
            // More of the paragraph or item that is open.
            Some((open_level, _, text)) if indent > 0 || *open_level == 0 => {
                text.push(' ');
                text.push_str(&plain(body));
            }
            _ => {
                close(&mut open, &mut out);
                in_list = in_list && level > 0;
                open = Some((level, None, plain(body)));
            }
        }
    }
    close(&mut open, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The words of `blocks`, with the marks that [`blocks`] turned into structure put back.
    fn rebuilt(blocks: &[Block]) -> String {
        let mut text = String::new();
        for block in blocks {
            let line = match block {
                Block::Title(title) => format!("# {title}"),
                Block::Heading(heading) => format!("## {heading}"),
                Block::Text { marker: Some(marker), text, .. } => format!("{marker} {text}"),
                Block::Text { marker: None, text, .. } => text.clone(),
                Block::Link { url, .. } => url.clone(),
                Block::Code(code) => code.clone(),
                Block::Rule => "---".to_string(),
            };
            text.push_str(&line);
            text.push('\n');
        }
        text
    }

    fn words(text: &str) -> Vec<&str> {
        text.split_whitespace().collect()
    }

    #[test]
    fn every_word_of_the_runbook_is_shown_once_and_in_order() {
        let source = runbook::SCRAM.replace('*', "");
        let shown = rebuilt(&SCRAM);
        assert_eq!(words(&shown), words(&source), "taking the text apart must neither drop nor add nor move a word");
        assert!(words(&shown).len() > 900, "the whole runbook, not an excerpt: {}", words(&shown).len());
    }

    #[test]
    fn the_addresses_set_apart_are_exactly_the_allowlist() {
        let mut shown: Vec<&str> = SCRAM
            .iter()
            .filter_map(|block| if let Block::Link { url, .. } = block { Some(url.as_str()) } else { None })
            .collect();
        shown.sort_unstable();
        shown.dedup();
        let mut listed: Vec<&str> = runbook::LINKS.iter().map(|(link, _)| *link).collect();
        listed.sort_unstable();
        assert_eq!(shown, listed);
        // And no address hides in the running text, where it would be shown without being
        // recognisable as one of them.
        for block in SCRAM.iter() {
            if let Block::Text { text, .. } | Block::Heading(text) | Block::Title(text) | Block::Code(text) = block {
                assert!(!text.contains("://"), "{text}");
            }
        }
    }

    #[test]
    fn the_structure_of_the_text_is_kept() {
        let blocks = &*SCRAM;
        assert!(matches!(&blocks[0], Block::Title(title) if title.starts_with("Emergency runbook")));
        let headings: Vec<&str> = blocks
            .iter()
            .filter_map(|block| if let Block::Heading(heading) = block { Some(heading.as_str()) } else { None })
            .collect();
        assert_eq!(headings.len(), 5, "{headings:?}");
        assert!(headings[0].starts_with("Read this first") && headings[1].starts_with("Phase 1"));
        // The warning about the device comes before the first step, as in the source.
        let position = |needle: &str| {
            blocks
                .iter()
                .position(|block| match block {
                    Block::Text { text, .. } | Block::Heading(text) => text.contains(needle),
                    _ => false,
                })
                .unwrap_or_else(|| panic!("{needle:?} is not in the text"))
        };
        assert!(position("from a different device") < position("Phase 1"));
        // The command, the numbered steps and the list inside the second step.
        assert!(blocks.contains(&Block::Code("bastion-mc scram".into())));
        let steps: Vec<&str> = blocks
            .iter()
            .filter_map(|block| match block {
                Block::Text { level: 1, marker: Some(marker), .. } if marker.ends_with('.') => Some(marker.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(steps, ["1.", "2.", "3.", "4.", "5."]);
        let nested =
            blocks.iter().filter(|block| matches!(block, Block::Text { level: 2, marker: Some(_), .. })).count();
        assert_eq!(nested, 3, "sign out everywhere, recovery code, sign-in methods");
        assert!(blocks.iter().any(|block| matches!(block,
            Block::Text { level: 2, text, .. } if text.starts_with("Sign out everywhere: Microsoft says") && text.ends_with("on the console itself."))));
        // An address inside a step stands at the step's depth; one outside the lists at none.
        assert!(blocks.contains(&Block::Link { level: 1, url: "https://www.microsoft.com/consent".into() }));
        assert!(
            blocks.contains(&Block::Link { level: 0, url: "https://help.minecraft.net/hc/en-us/requests/new".into() })
        );
        assert_eq!(blocks.iter().filter(|block| **block == Block::Rule).count(), 1);
        // No emphasis mark is left over.
        assert!(!rebuilt(blocks).contains('*'));
    }

    #[test]
    fn text_that_is_none_of_the_known_constructs_is_still_shown() {
        let odd =
            "plain\n  slightly indented\n\n        deep code\n1234. not a step\n-no space\n3. a step\nlazy line\n";
        let taken_apart = blocks(odd);
        assert_eq!(words(&rebuilt(&taken_apart)), words(odd));
        assert!(taken_apart.contains(&Block::Code("deep code".into())));
        assert!(taken_apart.contains(&Block::Text { level: 1, marker: Some("3.".into()), text: "a step".into() }));
        assert!(blocks("").is_empty());
    }
}
