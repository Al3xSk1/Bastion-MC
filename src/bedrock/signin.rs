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
//! **Wire format** (one request per connection): `u32` length, big-endian, then that many
//! bytes of a UTF-8 JSON object; at most [`MAX_FRAME`] bytes.
//! Request: `{"start": <url>, "end": <prefix>}`. Reply: `{"url": <final url>}` or
//! `{"error": <text>}`. The stub prints the URL and exits 0, or prints nothing and exits 1.
//!
//! **What the launcher accepts** (anything else is refused, logged once, and answered with an
//! error): a start URL `https://login.live.com/oauth20_authorize.srf?…` whose `client_id` is
//! [`CLIENT_ID`] and whose `redirect_uri` is [`REDIRECT_URI`], an end prefix equal to that
//! redirect, and as the final URL only a URL under that redirect. With PKCE, the code in the
//! final URL is useless without the verifier only the game holds.

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

use std::{
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
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
