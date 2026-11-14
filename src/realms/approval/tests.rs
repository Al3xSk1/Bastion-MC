use std::net::Ipv4Addr;

use super::*;

const PASS: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(127, 127, 255, 254), 25565);

fn target() -> Address {
    Address { host: "203.0.113.9".into(), port: 31234 }
}

type Heard = Arc<Mutex<Vec<Announcement>>>;

fn approvals(mode: Approve, totp: Option<TotpState>) -> (Arc<Approvals>, Heard) {
    let heard: Heard = Arc::default();
    let sink = heard.clone();
    let a = Approvals::new(mode, totp, PASS, Arc::new(move |a: &Announcement| sink.lock().unwrap().push(a.clone())));
    (a, heard)
}

fn last(heard: &Heard) -> Announcement {
    heard.lock().unwrap().last().cloned().expect("an announcement")
}

/// Ask for a join of `world` and let Realms answer: the prompt is up.
fn prompted(a: &Approvals, world: u64) {
    assert_eq!(a.join_requested(world), JoinStep::Forward);
    assert_eq!(
        a.prompt(world, target(), b"{\"address\":\"127.127.255.254:25565\"}".to_vec()),
        JoinStep::Wait(RETRY_AFTER_SECS)
    );
}

#[test]
fn a_code_approves_one_join_and_the_pass_lets_exactly_one_connection_through() {
    let (a, heard) = approvals(Approve::Code, None);
    a.note_names(br#"{"servers":[{"id":7,"name":"Our \u001b[31mRealm","owner":"x"},{"id":8}]}"#);
    assert_eq!(a.take_pass(), Err("no Realms join was approved"), "nothing before an approval");
    prompted(&a, 7);
    let shown = last(&heard);
    assert_eq!(shown.world, "Our \u{fffd}[31mRealm", "the name is shown, made printable");
    assert_eq!(shown.address, "203.0.113.9:31234");
    assert_eq!((shown.expires_in, shown.attempts_left, shown.outcome), (120, 3, None));
    let code = shown.code.clone().expect("code mode shows a code");
    assert!(code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit()));
    // The phone's state shows the prompt but never the code.
    let state = a.state();
    assert_eq!(state["mode"], "code");
    assert_eq!(state["prompt"]["world"], shown.world);
    assert_eq!(state["prompt"]["attempts_left"], 3);
    assert!(!state.to_string().contains(&code), "the code never goes through the game");
    // While it waits, the game is told to wait.
    assert_eq!(a.join_requested(7), JoinStep::Wait(RETRY_AFTER_SECS));
    assert_eq!(a.take_pass(), Err("no Realms join was approved"), "no pass before the answer is delivered");
    let wrong = if code == "000000" { "111111" } else { "000000" };
    assert_eq!(a.answer(wrong), Answer::Wrong { attempts_left: 2 });
    assert_eq!(a.answer(&code), Answer::Approved);
    assert_eq!(last(&heard).outcome, Some(Outcome::Approved));
    assert_eq!(last(&heard).code, None, "an ended prompt shows no code");
    assert_eq!(a.answer(&code), Answer::NoPrompt, "the code is spent");
    // The next join request gets the answer, once; the one after that is a new join.
    assert_eq!(a.join_requested(7), JoinStep::Deliver(b"{\"address\":\"127.127.255.254:25565\"}".to_vec()));
    assert_eq!(a.state()["last"], "approved");
    assert_eq!(a.take_pass(), Ok((target(), 7)));
    assert_eq!(a.take_pass(), Err("the approved Realms join was already used (one connection per approval)"));
    assert_eq!(a.join_requested(7), JoinStep::Forward, "joining again needs another approval");
}

#[test]
fn a_prompt_runs_out_after_120_seconds_and_three_wrong_codes_refuse_it() {
    let (a, heard) = approvals(Approve::Code, None);
    prompted(&a, 1);
    a.advance_clock(Duration::from_secs(119));
    assert_eq!(a.join_requested(1), JoinStep::Wait(RETRY_AFTER_SECS));
    a.advance_clock(Duration::from_secs(2));
    let code = heard.lock().unwrap()[0].code.clone().unwrap();
    assert_eq!(a.answer(&code), Answer::NoPrompt, "too late");
    assert_eq!(last(&heard).outcome, Some(Outcome::Expired));
    assert_eq!(a.state()["last"], "expired");
    assert!(matches!(a.join_requested(1), JoinStep::Refuse(_)), "a moment of rest after a prompt ran out");
    a.advance_clock(COOLDOWN);
    prompted(&a, 1);
    let code = last(&heard).code.unwrap();
    let wrong = if code == "123456" { "654321" } else { "123456" };
    assert_eq!(a.answer(wrong), Answer::Wrong { attempts_left: 2 });
    assert_eq!(a.answer("12345"), Answer::Wrong { attempts_left: 1 }, "malformed counts as an attempt");
    assert_eq!(a.answer(wrong), Answer::Refused);
    assert_eq!(last(&heard).outcome, Some(Outcome::Refused));
    assert_eq!(a.answer(&code), Answer::NoPrompt, "the right code after three wrong ones is too late");
    assert_eq!(a.take_pass(), Err("no Realms join was approved"));
}

#[test]
fn the_pass_is_good_for_a_short_window_only() {
    let (a, heard) = approvals(Approve::Code, None);
    prompted(&a, 3);
    assert_eq!(a.answer(&last(&heard).code.unwrap()), Answer::Approved);
    assert!(matches!(a.join_requested(3), JoinStep::Deliver(_)));
    a.advance_clock(PASS_WINDOW);
    assert_eq!(a.take_pass(), Err("the approved Realms join was not used in time"));
}

#[test]
fn one_prompt_at_a_time_cancel_and_a_bounded_number_per_session() {
    let (a, heard) = approvals(Approve::Code, None);
    prompted(&a, 1);
    assert!(matches!(a.join_requested(2), JoinStep::Refuse(why) if why.contains("another Realm")));
    // Two requests for the same join that raced: the second one waits.
    assert_eq!(a.prompt(1, target(), Vec::new()), JoinStep::Wait(RETRY_AFTER_SECS));
    assert!(a.cancel());
    assert_eq!(last(&heard).outcome, Some(Outcome::Refused));
    assert!(!a.cancel(), "nothing left to cancel");
    for _ in 1..MAX_PROMPTS {
        a.advance_clock(COOLDOWN);
        prompted(&a, 1);
        assert!(a.cancel());
    }
    a.advance_clock(COOLDOWN);
    assert!(matches!(a.join_requested(1), JoinStep::Refuse(why) if why.contains("no more")));
}

#[test]
fn a_totp_code_approves_once_and_is_never_accepted_again() {
    let secret = b"12345678901234567890";
    let record = std::env::temp_dir().join(format!("bmc-totp-last-{}", std::process::id()));
    let _ = std::fs::remove_file(&record);
    let state =
        TotpState { secret: Secret::from_slice(secret).unwrap(), last_used: None, record: Some(record.clone()) };
    let (a, heard) = approvals(Approve::Totp, Some(state));
    prompted(&a, 5);
    assert_eq!(last(&heard).code, None, "totp mode shows no code: the app has it");
    let now = a.unix_now();
    let code = totp::code_at(secret, now, 6);
    assert_eq!(a.answer(&code), Answer::Approved);
    assert_eq!(totp::load_last_used(&record), Some(now / totp::STEP), "the step is recorded for the next launch");
    assert!(matches!(a.join_requested(5), JoinStep::Deliver(_)));
    // The same code for the next prompt: refused as a replay.
    prompted(&a, 5);
    assert_eq!(a.answer(&code), Answer::Wrong { attempts_left: 2 });
    // The next step's code is fine.
    a.advance_clock(Duration::from_secs(totp::STEP));
    let next = totp::code_at(secret, a.unix_now(), 6);
    assert_eq!(a.answer(&next), Answer::Approved);
    std::fs::remove_file(&record).unwrap();

    // A step used in an earlier launch is not accepted either.
    let state =
        TotpState { secret: Secret::from_slice(secret).unwrap(), last_used: Some(now / totp::STEP + 5), record: None };
    let (b, _) = approvals(Approve::Totp, Some(state));
    prompted(&b, 5);
    assert_eq!(b.answer(&totp::code_at(secret, b.unix_now(), 6)), Answer::Wrong { attempts_left: 2 });
}

#[test]
fn wrong_totp_codes_are_bounded_per_session() {
    let state =
        TotpState { secret: Secret::from_slice(b"12345678901234567890").unwrap(), last_used: None, record: None };
    let (a, _) = approvals(Approve::Totp, Some(state));
    let mut wrong = 0;
    while wrong < MAX_TOTP_FAILURES {
        a.advance_clock(COOLDOWN);
        prompted(&a, 9);
        for _ in 0..ATTEMPTS {
            if wrong == MAX_TOTP_FAILURES {
                break;
            }
            // A code from far in the past is never right now.
            let stale = totp::code_at(b"12345678901234567890", 59, 6);
            assert!(matches!(a.answer(&stale), Answer::Wrong { .. } | Answer::Refused));
            wrong += 1;
        }
        a.cancel();
    }
    a.advance_clock(COOLDOWN);
    assert!(matches!(a.join_requested(9), JoinStep::Refuse(why) if why.contains("too many wrong codes")));
    assert_eq!(a.answer("123456"), Answer::Locked);
    assert_eq!(a.state()["locked"], true);
}
