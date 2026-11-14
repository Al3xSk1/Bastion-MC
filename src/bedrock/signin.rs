//! The sign-in bridge (owned by the sign-in package).
//!
//! The game's sign-in library asks `mcpelauncher-client` to show a Microsoft sign-in page;
//! the client runs a program called `mcpelauncher-webview <start-url> <end-url-prefix>` and
//! reads the final URL from its standard output. Inside bastion's sandbox that program is
//! bastion's **stub** ([`STUB_NAME`], mounted at [`SANDBOX_STUB`] and over every path where
//! the client could find an installed webview): it passes the two URLs over the bridge socket
//! ([`SANDBOX_SOCKET`]) to the launcher, which checks them, has the page shown outside the
//! game, and returns nothing but the final URL. The password, two-factor steps and cookies
//! never enter the game's sandbox, and a hostile game cannot make the launcher open anything
//! but Microsoft's sign-in page for this one client.
//!
//! **Wire format** (one request per connection, [`wire`]): `u32` length, big-endian, then that
//! many bytes of a UTF-8 JSON object; at most [`MAX_FRAME`] bytes.
//! Request: `{"start": <url>, "end": <prefix>}`. Reply: `{"url": <final url>}` or
//! `{"error": <text>}`. The stub prints the URL and exits 0, or prints nothing and exits 1
//! ([`stub`]); it keeps the connection open while it waits, and a closed connection
//! withdraws the prompt.
//!
//! **What the launcher accepts** ([`check`]; anything else is refused, logged once per reason,
//! and answered with an error): a start URL `https://login.live.com/oauth20_authorize.srf?…`
//! whose `client_id` is [`CLIENT_ID`], whose `redirect_uri` is [`REDIRECT_URI`], whose
//! `response_type` is `code`, whose `scope` is Xbox sign-in alone ([`check::SCOPE`]), with a
//! PKCE challenge (S256) and a state; an end prefix equal to that redirect; and as the final
//! URL only a URL under that redirect with a code and the request's state, which goes back
//! rebuilt from those two values. With PKCE, the code in the final URL is useless without
//! the verifier only the game holds.
//!
//! **Who answers** ([`server`]): the page is offered to the user outside the game — an event
//! for the front end (which opens it in the user's browser) and a printed URL on a terminal
//! launch — and the final URL comes back only from trusted input (`events::input`: the front
//! end's control pipe or the terminal), never from the game. One prompt at a time, a few per
//! session, each for at most 15 minutes.

/// The program name the client looks for.
pub const STUB_NAME: &str = "mcpelauncher-webview";

/// Where the stub is mounted in the sandbox (the sandbox's `PATH` starts with its directory).
pub const SANDBOX_STUB: &str = "/bastion/bin/mcpelauncher-webview";

/// The directory the launch binds [`Bridge::socket_dir`] to, and the socket in it.
pub const SANDBOX_RUN_DIR: &str = "/bastion/run";
pub const SANDBOX_SOCKET: &str = "/bastion/run/signin.sock";
/// The socket's file name in [`Bridge::socket_dir`].
pub const SOCKET_NAME: &str = "signin.sock";

/// Minecraft for Android's Microsoft client id: the only client the bridge signs in.
pub const CLIENT_ID: &str = "0000000048183522";

/// Its redirect, which is also the only accepted end prefix.
pub const REDIRECT_URI: &str = "ms-xal-0000000048183522://auth";

/// The only accepted start of a sign-in page's address.
pub const START_PREFIX: &str = "https://login.live.com/oauth20_authorize.srf?";

/// The largest frame in either direction.
pub const MAX_FRAME: usize = 16 * 1024;

pub mod check;
pub mod server;
pub mod stub;
pub mod wire;

use std::{
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
};

pub use self::{
    server::{Serving, serve},
    stub::is_stub,
};
use crate::error::{Error, Result};

/// The launcher's end of the bridge. Created before the launcher confines itself (binding a
/// pathname socket is not allowed after that); accepting on it afterwards is.
#[derive(Debug)]
pub struct Bridge {
    dir: PathBuf,
    listener: UnixListener,
}

impl Bridge {
    /// Binds `<runtime_dir>/signin.sock`. `runtime_dir` is a private directory (0700) of this
    /// launch that holds nothing else the game may see: the launch binds it at
    /// [`SANDBOX_RUN_DIR`].
    pub fn prepare(runtime_dir: &Path) -> Result<Bridge> {
        let path = runtime_dir.join(SOCKET_NAME);
        let listener = UnixListener::bind(&path).map_err(|e| Error::io(format!("binding {}", path.display()), e))?;
        Ok(Bridge { dir: runtime_dir.to_path_buf(), listener })
    }

    /// The host directory to bind at [`SANDBOX_RUN_DIR`].
    pub fn socket_dir(&self) -> &Path {
        &self.dir
    }

    /// The listening socket, for the sign-in package's server.
    pub fn into_listener(self) -> UnixListener {
        self.listener
    }
}

/// Fuzz harness: bytes from the sandbox as the launcher reads them — a frame header, a
/// request body, the URLs in it — through every check a request passes. Whatever the bytes,
/// nothing panics, no frame outside the bounds is read, and a request that passes asks for a
/// page on login.live.com that the front end's link check accepts.
#[doc(hidden)]
pub fn fuzz_frame(data: &[u8]) {
    let body = match data.first_chunk::<4>().map(|h| wire::body_len(*h)) {
        Some(Ok(len)) => {
            assert!((2..=MAX_FRAME).contains(&len));
            &data[4..data.len().min(4 + len)]
        }
        _ => data,
    };
    if let Ok(request) = wire::parse_request(body) {
        // Its own frame reads back to the same request.
        if let Some(framed) = wire::request_frame(&request.start, &request.end) {
            let again = wire::read_frame_blocking(&mut framed.as_slice()).expect("own frame");
            assert_eq!(wire::parse_request(&again).expect("own request"), request);
        }
        if let Ok(checked) = check::check_request(&request.start, &request.end) {
            assert_checked(&checked);
        }
    }
    if let Ok(wire::Reply::Url(url) | wire::Reply::Error(url)) = wire::parse_reply(body) {
        assert!(url.len() <= MAX_FRAME);
    }
}

/// Fuzz harness: the URL checks on their own, with the bytes as the start address and as the
/// final address pasted for a sign-in. What passes is bounded, printable, under the right
/// prefix, carries the state it must, and passes again unchanged.
#[doc(hidden)]
pub fn fuzz_url(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    if let Ok(checked) = check::check_request(&text, REDIRECT_URI) {
        assert_checked(&checked);
    }
    // The same bytes behind the prefixes, so that mutation explores the queries.
    let start = format!("{START_PREFIX}{text}");
    if let Ok(checked) = check::check_request(&start, REDIRECT_URI) {
        assert_checked(&checked);
    }
    let request = check::Checked::for_test("Zm9vYmFy-_.~");
    for answer in [text.to_string(), format!("{REDIRECT_URI}?{text}")] {
        if let Ok(url) = check::check_final(&answer, &request) {
            assert!(url.len() <= check::MAX_FINAL && url.bytes().all(|b| b.is_ascii_graphic()), "{url:?}");
            assert!(url.starts_with(&format!("{REDIRECT_URI}?code=")), "{url:?}");
            assert_eq!(check::check_final(&url, &request).as_deref(), Ok(url.as_str()), "not a fixed point");
        }
    }
}

fn assert_checked(checked: &check::Checked) {
    let url = &checked.url;
    assert!(url.starts_with(START_PREFIX) && url.len() <= check::MAX_START, "{url:?}");
    assert!(url.bytes().all(|b| b.is_ascii_graphic()) && !url.contains('#'), "{url:?}");
}
