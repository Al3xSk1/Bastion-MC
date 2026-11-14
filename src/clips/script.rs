//! Test support: phone commands that a launch sends to its own apps after step 9, as if the
//! phone had sent them, with everything the apps send back written to the launch log.
//!
//! Only reachable from code (`ClipsConfig::script` cannot be set in a configuration file):
//! it lets a test drive the media apps through a real launch — the real game, the real
//! recorder, the confined launcher — without the companion mod's phone. Commands go through
//! `phone::Command::parse` and the app's own verb list, as the dispatcher would send them;
//! the dispatcher's rate limits are not applied here (the apps' own limits are).

use std::time::Duration;

use super::Sink;
use crate::phone::{Command, Outbound, Outbox, PhoneApp};

/// One command: sent `after` the previous one (or the start).
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub after: Duration,
    /// A phone command line; `{last-clip}` is replaced by the id of the newest clip in the
    /// last state the clips app sent.
    pub line: String,
}

/// How each message reads in the log (bounded: the log takes lines of up to 4 KiB).
fn describe(message: &Outbound) -> String {
    match message {
        Outbound::State { app, json } => format!("< {app} state {}", crate::util::sanitize_line(json.as_bytes(), 3000)),
        Outbound::Frame { app, slot, width, height, format, bytes } => {
            format!("< {app} frame slot={slot} {width}x{height} {format:?} {} bytes", bytes.len())
        }
        Outbound::Notice { app, text } => {
            format!("< {app} notice {}", crate::util::sanitize_line(text.as_bytes(), 300))
        }
    }
}

/// Send `steps` to `apps` (after saying hello to each); keeps the apps until it is aborted.
pub async fn drive(mut apps: Vec<Box<dyn PhoneApp>>, steps: Vec<Step>, log: Sink) {
    let (out, mut rx) = Outbox::new(256);
    let newest = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let (seen, sink) = (newest.clone(), log.clone());
    let reader = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if let Outbound::State { app: "clips", json } = &message
                && let Ok(value) = serde_json::from_str::<serde_json::Value>(json)
                && let Some(id) = value["clips"][0]["id"].as_str()
            {
                *seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(id.to_string());
            }
            sink(format!("phone script: {}", describe(&message)));
        }
    });
    for app in apps.iter_mut() {
        app.hello(&out);
    }
    for step in steps {
        tokio::time::sleep(step.after).await;
        let id = newest.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone().unwrap_or_default();
        let line = step.line.replace("{last-clip}", &id);
        log(format!("phone script: > {line}"));
        let command = match Command::parse(line.as_bytes()) {
            Ok(command) => command,
            Err(why) => {
                log(format!("phone script: refused ({})", why.as_str()));
                continue;
            }
        };
        match apps.iter_mut().find(|a| a.name() == command.app) {
            Some(app) if app.limits().iter().any(|l| l.verb == command.verb) => app.command(command, &out),
            Some(_) => log("phone script: refused (not a verb of that app)".into()),
            None => log("phone script: refused (no such app)".into()),
        }
    }
    log("phone script: done".into());
    // The apps (and with them the recorder) live until the launch ends and aborts this.
    std::future::pending::<()>().await;
    drop((apps, reader));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phone::FrameFormat;

    #[test]
    fn messages_are_logged_bounded() {
        let state = Outbound::State { app: "clips", json: format!("{{\"x\":\"{}\"}}", "y".repeat(5000)) };
        assert!(describe(&state).len() < 3100);
        let frame = Outbound::Frame {
            app: "clips",
            slot: 2,
            width: 320,
            height: 180,
            format: FrameFormat::Png,
            bytes: vec![0; 10],
        };
        assert_eq!(describe(&frame), "< clips frame slot=2 320x180 Png 10 bytes");
    }
}
