//! The console's contents: a bounded window onto the launcher's and the game's output.
//!
//! The complete log is the launcher's file on disk. This is what is worth keeping on screen:
//! the most recent lines, capped so that a chatty game cannot grow the UI's memory or slow
//! its layout down.
//!
//! Modern Minecraft does not print plain lines: its logging configuration (meant for
//! Mojang's launcher) emits every message as a small log4j XML document spread over three
//! or more lines. Those are folded back into the one line a person wants to read,
//! `[thread/LEVEL] message`. Folding never drops anything: output that does not look like
//! such an event is shown exactly as it arrived.

use std::collections::VecDeque;

use bastion_mc_proto::Source;

/// Lines kept (and laid out when the console is open).
pub const CAPACITY: usize = 800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub source: Source,
    pub level: Level,
    pub text: String,
}

/// Where the folder is inside a log4j XML event.
#[derive(Debug, Default)]
enum Fold {
    #[default]
    Outside,
    /// Between `<log4j:Event …>` and `</log4j:Event>`.
    Event { prefix: String, level: Level },
    /// Inside a multi-line `<![CDATA[ … ]]>` block of that event (`closing` ends it).
    Block { prefix: String, level: Level, closing: &'static str },
}

#[derive(Debug, Default)]
pub struct Console {
    lines: VecDeque<Line>,
    /// Lines that scrolled out of the window since the last clear.
    discarded: u64,
    fold: Fold,
    pub open: bool,
}

const EVENT_OPEN: &str = "<log4j:Event ";
const EVENT_CLOSE: &str = "</log4j:Event>";
const BLOCKS: [(&str, &str); 2] =
    [("<log4j:Message><![CDATA[", "]]></log4j:Message>"), ("<log4j:Throwable><![CDATA[", "]]></log4j:Throwable>")];

/// The value of `name="…"` in an XML start tag, with the five predefined entities resolved.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let start = tag.find(&format!(" {name}=\""))? + name.len() + 3;
    let value = &tag[start..start + tag[start..].find('"')?];
    Some(
        value
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}

fn level_of(name: &str) -> Level {
    match name {
        "WARN" => Level::Warn,
        "ERROR" | "FATAL" => Level::Error,
        _ => Level::Info,
    }
}

impl Console {
    fn append(&mut self, source: Source, level: Level, text: String) {
        if self.lines.len() == CAPACITY {
            self.lines.pop_front();
            self.discarded += 1;
        }
        self.lines.push_back(Line { source, level, text });
    }

    /// Add a line of output. Game output is folded (see the module documentation); a denied
    /// connection reported by the egress relay is marked as a warning.
    pub fn push(&mut self, source: Source, text: impl Into<String>) {
        let text = text.into();
        match source {
            Source::Game => self.push_game(text),
            Source::Egress => {
                let level = if text.contains("DENY") { Level::Warn } else { Level::Info };
                self.append(source, level, text);
            }
            Source::Launcher => self.append(source, Level::Info, text),
        }
    }

    /// Add a launcher line with an explicit level (errors, security notices).
    pub fn push_with(&mut self, source: Source, level: Level, text: impl Into<String>) {
        self.append(source, level, text.into());
    }

    fn push_game(&mut self, raw: String) {
        let line = raw.trim();
        match std::mem::take(&mut self.fold) {
            Fold::Outside => {
                if line.starts_with(EVENT_OPEN) && line.ends_with('>') {
                    let level_name = attribute(line, "level").unwrap_or_default();
                    let thread = attribute(line, "thread").unwrap_or_default();
                    self.fold =
                        Fold::Event { prefix: format!("[{thread}/{level_name}]"), level: level_of(&level_name) };
                } else {
                    // Plain output (older versions, native libraries, the JVM itself).
                    self.append(Source::Game, Level::Info, raw);
                }
            }
            Fold::Event { prefix, level } => {
                if line == EVENT_CLOSE {
                    return; // back outside
                }
                let block = BLOCKS.iter().find(|(open, _)| line.starts_with(open));
                match block {
                    Some((open, closing)) => {
                        let body = &line[open.len()..];
                        // A throwable continues its event's message; it gets no prefix of its own.
                        let lead = if open.contains("Message") { format!("{prefix} ") } else { String::new() };
                        match body.strip_suffix(closing) {
                            Some(whole) => {
                                if !(whole.is_empty() && lead.is_empty()) {
                                    self.append(Source::Game, level, format!("{lead}{whole}"));
                                }
                                self.fold = Fold::Event { prefix, level };
                            }
                            None => {
                                self.append(Source::Game, level, format!("{lead}{body}"));
                                self.fold = Fold::Block { prefix, level, closing };
                            }
                        }
                    }
                    None => {
                        // Something this folder does not know: show it, stay in the event.
                        self.append(Source::Game, level, raw);
                        self.fold = Fold::Event { prefix, level };
                    }
                }
            }
            Fold::Block { prefix, level, closing } => match line.strip_suffix(closing) {
                Some(last) => {
                    if !last.is_empty() {
                        self.append(Source::Game, level, last.to_string());
                    }
                    self.fold = Fold::Event { prefix, level };
                }
                None => {
                    // Keep the original indentation of stack traces and multi-line messages.
                    self.append(Source::Game, level, raw.trim_end().to_string());
                    self.fold = Fold::Block { prefix, level, closing };
                }
            },
        }
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.discarded = 0;
    }

    /// A new game session starts: whatever the previous one left half-said is forgotten.
    pub fn new_session(&mut self) {
        self.fold = Fold::Outside;
    }

    pub fn lines(&self) -> impl Iterator<Item = &Line> {
        self.lines.iter()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn discarded(&self) -> u64 {
        self.discarded
    }

    /// Everything on screen as plain text, for the clipboard.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for line in &self.lines {
            out.push('[');
            out.push_str(line.source.tag());
            out.push_str("] ");
            out.push_str(&line.text);
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(lines: &[&str]) -> Vec<(Level, String)> {
        let mut console = Console::default();
        for line in lines {
            console.push(Source::Game, *line);
        }
        console.lines().map(|l| (l.level, l.text.clone())).collect()
    }

    #[test]
    fn keeps_only_the_most_recent_lines() {
        let mut console = Console::default();
        for i in 0..CAPACITY + 25 {
            console.push(Source::Game, format!("line {i}"));
        }
        assert_eq!(console.len(), CAPACITY);
        assert_eq!(console.discarded(), 25);
        assert_eq!(console.lines().next().unwrap().text, "line 25");
        assert_eq!(console.lines().last().unwrap().text, format!("line {}", CAPACITY + 24));
        console.clear();
        assert!(console.is_empty() && console.discarded() == 0);
    }

    #[test]
    fn clipboard_text_is_tagged_by_source() {
        let mut console = Console::default();
        console.push(Source::Launcher, "sandbox started");
        console.push(Source::Game, "Setting user: Player");
        console.push(Source::Egress, "egress: ALLOW sessionserver.mojang.com:443");
        assert_eq!(
            console.to_text(),
            "[launcher] sandbox started\n[game] Setting user: Player\n[egress] egress: ALLOW sessionserver.mojang.com:443\n"
        );
    }

    #[test]
    fn log4j_events_are_folded_into_one_line_each() {
        // Verbatim from a 26.3 client (indentation included).
        let folded = game(&[
            r#"  <log4j:Event logger="oshi.util.FileUtil" timestamp="1794570810672" level="WARN" thread="main">"#,
            r#"    <log4j:Message><![CDATA[File not found or not readable: /etc/os-release]]></log4j:Message>"#,
            r#"  </log4j:Event>"#,
            r#"<log4j:Event logger="net.minecraft.client.Minecraft" timestamp="1794570811000" level="INFO" thread="Render thread">"#,
            r#"	<log4j:Message><![CDATA[Setting user: Player]]></log4j:Message>"#,
            r#"</log4j:Event>"#,
        ]);
        assert_eq!(
            folded,
            vec![
                (Level::Warn, "[main/WARN] File not found or not readable: /etc/os-release".to_string()),
                (Level::Info, "[Render thread/INFO] Setting user: Player".to_string()),
            ]
        );
    }

    #[test]
    fn multi_line_messages_and_stack_traces_keep_every_line() {
        let folded = game(&[
            r#"<log4j:Event logger="x" timestamp="1" level="ERROR" thread="Server thread">"#,
            r#"<log4j:Message><![CDATA[Encountered an unexpected exception"#,
            r#"second line of the message]]></log4j:Message>"#,
            r#"<log4j:Throwable><![CDATA[java.lang.IllegalStateException: boom"#,
            "\tat net.minecraft.Example.run(Example.java:42)",
            r#"]]></log4j:Throwable>"#,
            r#"</log4j:Event>"#,
            "plain line after the event",
        ]);
        assert_eq!(
            folded,
            vec![
                (Level::Error, "[Server thread/ERROR] Encountered an unexpected exception".to_string()),
                (Level::Error, "second line of the message".to_string()),
                (Level::Error, "java.lang.IllegalStateException: boom".to_string()),
                (Level::Error, "\tat net.minecraft.Example.run(Example.java:42)".to_string()),
                (Level::Info, "plain line after the event".to_string()),
            ]
        );
    }

    #[test]
    fn anything_else_is_shown_as_it_arrived() {
        // Old versions and native code print plain text; nothing may be swallowed.
        let plain =
            ["[12:00:00] [Client thread/INFO]: LWJGL Version: 2.9.4", "<notlog4j>", "  indented", "</log4j:Event>"];
        let folded = game(&plain);
        assert_eq!(folded.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(), plain);

        // An event that never closes (the game died mid-line) still shows what it said, and
        // unknown elements inside an event are shown rather than dropped.
        let folded = game(&[
            r#"<log4j:Event logger="x" timestamp="1" level="FATAL" thread="main">"#,
            "<log4j:Unknown>kept</log4j:Unknown>",
            "<log4j:Message><![CDATA[half a mess",
        ]);
        assert_eq!(
            folded,
            vec![
                (Level::Error, "<log4j:Unknown>kept</log4j:Unknown>".to_string()),
                (Level::Error, "[main/FATAL] half a mess".to_string()),
            ]
        );
    }

    #[test]
    fn a_new_session_does_not_inherit_a_half_open_event() {
        let mut console = Console::default();
        console.push(Source::Game, r#"<log4j:Event logger="x" timestamp="1" level="ERROR" thread="main">"#);
        console.new_session();
        console.push(Source::Game, "fresh start");
        assert_eq!(console.lines().last().unwrap().level, Level::Info);
    }

    #[test]
    fn attributes_are_unescaped_and_missing_ones_tolerated() {
        assert_eq!(
            attribute(r#"<e thread="A &amp; B &lt;1&gt;" level="INFO">"#, "thread").as_deref(),
            Some("A & B <1>")
        );
        assert_eq!(attribute(r#"<e level="INFO">"#, "thread"), None);
        assert_eq!(attribute(r#"<e thread="unterminated>"#, "thread"), None);
        let folded = game(&[r#"<log4j:Event timestamp="1">"#, "<log4j:Message><![CDATA[x]]></log4j:Message>"]);
        assert_eq!(folded, vec![(Level::Info, "[/] x".to_string())]);
    }

    #[test]
    fn levels_of_other_sources() {
        let mut console = Console::default();
        console.push(Source::Egress, "egress: ALLOW api.minecraftservices.com:443");
        console.push(Source::Egress, "egress: DENY  name lookup \"evil.example\" (not allowlisted)");
        console.push_with(Source::Launcher, Level::Error, "error: INTEGRITY VIOLATION");
        let levels: Vec<Level> = console.lines().map(|l| l.level).collect();
        assert_eq!(levels, vec![Level::Info, Level::Warn, Level::Error]);
    }
}
