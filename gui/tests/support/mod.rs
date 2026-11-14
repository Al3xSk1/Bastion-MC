//! What the end-to-end tests share: the application run by iced's emulator, and the means
//! to act on it and to wait for what follows.

// Each test file uses a part of this.
#![allow(dead_code)]

use std::{
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

use bastion_mc_gui::{WINDOW_SIZE, theme::caps};
use iced::futures::channel::mpsc;
use iced_test::{
    Emulator, Instruction,
    emulator::{Event, Mode},
    program::Program,
};

/// One emulator at a time. Each has a headless renderer of its own, and the emulator picks
/// the GPU one where there is a GPU (no window is involved). Four of those starting up at
/// once on different test threads crashed the test process on the development machine: a
/// SIGSEGV in the Vulkan loader, under `vkEnumerateInstanceExtensionProperties`.
static EMULATOR: Mutex<()> = Mutex::new(());

/// Take the turn to run an emulator. Whoever holds it is also the only test of its file
/// that starts processes.
pub fn one_at_a_time() -> MutexGuard<'static, ()> {
    EMULATOR.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// No step against a stand-in launcher takes anywhere near this long; a hang must still
/// end the run.
pub const PATIENCE: Duration = Duration::from_secs(30);

/// A running application and the means to act on it.
///
/// The emulator never waits here ([`Mode::Immediate`]): a sign-in or a running game is a
/// task that does not end by itself, and the tests have to act while it is pending. What a
/// step leads to is therefore waited for explicitly, with [`Session::eventually`].
pub struct Session<P: Program + 'static> {
    program: P,
    emulator: Emulator<P>,
    events: mpsc::Receiver<Event<P>>,
    /// Released last, after the emulator and its renderer are gone.
    _one_at_a_time: MutexGuard<'static, ()>,
}

impl<P: Program + 'static> Session<P> {
    /// Boot `program` in a window of the size the real one opens with.
    pub fn boot(program: P, one_at_a_time: MutexGuard<'static, ()>) -> Self {
        let (sender, events) = mpsc::channel(64);
        let emulator = Emulator::new(sender, &program, Mode::Immediate, WINDOW_SIZE);
        let mut session = Session { program, emulator, events, _one_at_a_time: one_at_a_time };
        assert!(session.settle(), "the application boots");
        session
    }

    /// Let the application work until the emulator reports on the instruction it was given
    /// last: `true` if it could be carried out.
    fn settle(&mut self) -> bool {
        let deadline = Instant::now() + PATIENCE;
        loop {
            match self.events.try_recv() {
                Ok(Event::Action(action)) => self.emulator.perform(&self.program, action),
                Ok(Event::Ready) => return true,
                Ok(Event::Failed(_)) => return false,
                Err(_) => {
                    assert!(Instant::now() < deadline, "the emulator did not answer within {PATIENCE:?}");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }

    fn run(&mut self, step: &str) -> bool {
        let instruction = Instruction::parse(step).unwrap_or_else(|e| panic!("bad step {step:?}: {e}"));
        self.emulator.run(&self.program, instruction);
        self.settle()
    }

    /// Is this exact piece of text on screen right now?
    pub fn shows(&mut self, text: &str) -> bool {
        self.run(&format!("expect \"{text}\""))
    }

    /// Wait until `text` is on screen.
    pub fn eventually(&mut self, text: &str) {
        self.eventually_within(text, PATIENCE);
    }

    /// [`Session::eventually`], for what takes its time (a real game starting).
    pub fn eventually_within(&mut self, text: &str, patience: Duration) {
        let deadline = Instant::now() + patience;
        while !self.shows(text) {
            assert!(Instant::now() < deadline, "{text:?} did not appear within {patience:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Press the control with this text on it. It has to be there.
    pub fn click(&mut self, text: &str) {
        assert!(self.run(&format!("click \"{text}\"")), "nothing to click that says {text:?}");
    }

    /// Wait until the launcher has nothing left to do: the activity read-out at the top of
    /// every page says so.
    pub fn idle(&mut self) {
        self.eventually(&caps("Ready"));
    }
}
