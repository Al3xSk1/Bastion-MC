//! Realms approvals answered outside the game, for a session without the phone (Bedrock).
//!
//! In a Java session the user types the approval code into the phone's Realms app, in the
//! game; the code proves that the person typing saw the prompt outside it. A Bedrock session
//! has no phone. There the answer comes from the **trusted input** instead
//! ([`crate::events::input`]): `answer realms <code>` (or `answer realms cancel`) from the front
//! end, or the code typed on the terminal of a terminal launch. The game cannot write to
//! either, so the answer is the user's by construction; the code still binds it to the one
//! prompt it was shown with (`code` mode), or is the authenticator's (`totp` mode), and every
//! bound of [`approval`] — attempts, prompts per session, the lock after wrong codes — holds
//! unchanged.
//!
//! Use, by whoever builds the session's [`Approvals`]:
//!
//! ```ignore
//! let answers = reporter.answers(AnswerKind::Realms)?;           // trusted input, kind `realms`
//! let announce = realms::answers::tracking(announce, answers.pending_handle());
//! let approvals = Approvals::new(mode, totp, pass_address, announce);
//! let answering = realms::answers::serve(answers, approvals.clone(), reporter.clone());
//! // … at the end of the session: answering.abort()
//! ```
//!
//! The front end learns from [`Event::Answerable`](crate::events::Event) that Realms prompts are
//! answered here, and shows an input with the prompt; a terminal launch prints "type this code
//! here" instead of "into the phone".

use std::sync::Arc;

use super::approval::{self, Announcement, Announcer, Approvals};
use crate::events::{Answer, AnswerReceiver, PendingHandle, Reporter};

/// An announcer that also tells the trusted input whether a prompt waits — so that a code
/// typed on the terminal goes to it without naming its kind — and then announces as before.
pub fn tracking(announce: Announcer, pending: PendingHandle) -> Announcer {
    Arc::new(move |a: &Announcement| {
        pending.set(a.outcome.is_none());
        announce(a);
    })
}

/// What the user is told about an answer the prompt's own announcement does not cover.
pub fn note(answer: &approval::Answer) -> Option<String> {
    match answer {
        // Approved and refused prompts are announced by `Approvals` itself.
        approval::Answer::Approved | approval::Answer::Refused => None,
        approval::Answer::Wrong { attempts_left } => {
            Some(format!("Realms: that is not the code; {attempts_left} attempt(s) left"))
        }
        approval::Answer::NoPrompt => Some("Realms: no join is waiting for approval; that code was ignored".into()),
        approval::Answer::Locked => {
            Some("Realms: too many wrong codes in this session; joins are refused until the next launch".into())
        }
    }
}

/// Hand every Realms answer from the trusted input to `approvals`, for the rest of the
/// session (or until the task is aborted). Must be called inside the launch's runtime.
pub fn serve(
    mut answers: AnswerReceiver,
    approvals: Arc<Approvals>,
    reporter: Reporter,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(answer) = answers.recv().await {
            let text = match answer {
                Answer::Cancel if approvals.cancel() => None,
                Answer::Cancel => Some("Realms: no join is waiting for approval; nothing to cancel".to_string()),
                Answer::Text(code) => note(&approvals.answer(&code)),
            };
            if let Some(text) = text {
                reporter.warn(text);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::{
        net::{Ipv4Addr, SocketAddrV4},
        sync::Mutex,
        time::{Duration, Instant},
    };

    use super::*;
    use crate::{
        events::{AnswerKind, Delivery, Event},
        realms::{Approve, approval::JoinStep, table::Address},
    };

    fn approvals_for(reporter: &Reporter) -> (Arc<Approvals>, Arc<Mutex<Vec<Announcement>>>, AnswerReceiver) {
        let answers = reporter.answers(AnswerKind::Realms).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let announce: Announcer = Arc::new(move |a: &Announcement| sink.lock().unwrap().push(a.clone()));
        let announce = tracking(announce, answers.pending_handle());
        let approvals =
            Approvals::new(Approve::Code, None, SocketAddrV4::new(Ipv4Addr::new(127, 64, 0, 9), 25565), announce);
        (approvals, seen, answers)
    }

    async fn eventually(what: &str, check: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(Instant::now() < deadline, "{what}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn a_code_from_the_trusted_input_approves_the_join_it_was_shown_with() {
        let reporter = Reporter::capture();
        let (approvals, seen, answers) = approvals_for(&reporter);
        assert!(reporter.captured().contains(&Event::Answerable { kind: AnswerKind::Realms }));
        let task = serve(answers, approvals.clone(), reporter.clone());
        assert_eq!(approvals.join_requested(7), JoinStep::Forward);
        let target = Address { host: "203.0.113.9".into(), port: 19132 };
        assert_eq!(approvals.prompt(7, target, b"{}".to_vec()), JoinStep::Wait(3));
        let code = seen.lock().unwrap()[0].code.clone().expect("code mode shows a code");
        // A wrong code costs an attempt and says so; the right one approves.
        assert_eq!(reporter.answer_for_test(AnswerKind::Realms, Answer::Text("000000x".into())), Delivery::Delivered);
        eventually("the wrong code is reported", || {
            reporter
                .captured()
                .iter()
                .any(|e| matches!(e, Event::Status { text } if text.contains("2 attempt(s) left")))
        })
        .await;
        assert_eq!(reporter.answer_for_test(AnswerKind::Realms, Answer::Text(code)), Delivery::Delivered);
        eventually("the join is approved", || {
            seen.lock().unwrap().iter().any(|a| a.outcome == Some(approval::Outcome::Approved))
        })
        .await;
        assert_eq!(approvals.join_requested(7), JoinStep::Deliver(b"{}".to_vec()));
        task.abort();
    }

    #[tokio::test]
    async fn cancel_refuses_the_waiting_join_and_says_when_there_is_none() {
        let reporter = Reporter::capture();
        let (approvals, seen, answers) = approvals_for(&reporter);
        let task = serve(answers, approvals.clone(), reporter.clone());
        reporter.answer_for_test(AnswerKind::Realms, Answer::Cancel);
        eventually("nothing to cancel is said", || {
            reporter
                .captured()
                .iter()
                .any(|e| matches!(e, Event::Status { text } if text.contains("nothing to cancel")))
        })
        .await;
        approvals.join_requested(3);
        approvals.prompt(3, Address { host: "203.0.113.9".into(), port: 19132 }, Vec::new());
        reporter.answer_for_test(AnswerKind::Realms, Answer::Cancel);
        eventually("the join is refused", || {
            seen.lock().unwrap().iter().any(|a| a.outcome == Some(approval::Outcome::Refused))
        })
        .await;
        task.abort();
    }

    #[test]
    fn the_terminal_knows_while_a_realms_prompt_waits() {
        let registry = crate::events::input::Registry::new();
        let answers = registry.subscribe(AnswerKind::Realms).unwrap();
        let quiet: Announcer = Arc::new(|_: &Announcement| {});
        let approvals = Approvals::new(
            Approve::Code,
            None,
            SocketAddrV4::new(Ipv4Addr::new(127, 64, 0, 9), 25565),
            tracking(quiet, answers.pending_handle()),
        );
        assert!(registry.pending().is_empty());
        approvals.join_requested(1);
        approvals.prompt(1, Address { host: "203.0.113.9".into(), port: 19132 }, Vec::new());
        assert_eq!(registry.pending(), [AnswerKind::Realms], "a bare typed code now goes to the prompt");
        assert!(approvals.cancel());
        assert!(registry.pending().is_empty(), "and no longer once it ended");
    }
}
