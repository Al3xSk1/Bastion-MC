//! `bastion-mc browser login <site>`: sign in to a site outside the game.
//!
//! The same sandbox, profile and relay as the in-game browser, but as a visible window on
//! your desktop and without the remote control: you type your password into Firefox
//! directly, never through the game. When you close the window, the session the site left
//! in the profile (its cookies and storage) is what the in-game browser starts with.
//!
//! The window can reach the site's hosts and nothing else, like the in-game browser. What
//! differs: it is a client of your display (on X11 a client of your X server, like any other
//! window there; on Wayland a client of your compositor), and nothing in the game is
//! involved. Firefox does not offer to save the password (`signon.rememberSignons` is off,
//! and the enterprise policy locks the password manager off), so the profile holds the
//! site's session, not your password.

use std::sync::Arc;

use tokio::sync::watch;

use super::{
    relay::{self, SiteState, Upstream},
    sandbox::{self, Desktop, Mode, Plan},
};
use crate::{
    config::Config,
    error::{Error, Result},
    events::Reporter,
    paths::Paths,
};

/// Run the login window for the site named `name` on the session's display until it is
/// closed (or Ctrl-C).
pub async fn run(
    paths: &Paths,
    cfg: &Config,
    name: &str,
    program: &std::path::Path,
    reporter: &Reporter,
) -> Result<i32> {
    // The site is checked before anything is asked of the display.
    site_named(cfg, name)?;
    run_on(paths, cfg, name, program, reporter, Desktop::from_session(paths)?).await
}

fn site_named(cfg: &Config, name: &str) -> Result<super::config::Site> {
    let sites = cfg.browser.resolved_sites()?;
    sites.iter().find(|s| s.name == name).cloned().ok_or_else(|| {
        Error::Usage(if sites.is_empty() {
            "[browser] lists no sites; add one (for example `sites = [{ preset = \"discord\" }]`) first".into()
        } else {
            format!(
                "there is no site {name:?} in [browser] sites (listed: {})",
                sites.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
            )
        })
    })
}

/// [`run`] on a given display (the tests use a private one).
pub async fn run_on(
    paths: &Paths,
    cfg: &Config,
    name: &str,
    program: &std::path::Path,
    reporter: &Reporter,
    desktop: Desktop,
) -> Result<i32> {
    let site = site_named(cfg, name)?;
    let upstream = if !cfg.browser.test_upstreams.is_empty() {
        Upstream::Fixed(cfg.browser.test_upstreams.clone())
    } else {
        match cfg.network.launcher_proxy.as_deref().map(crate::config::parse_socks5).transpose()? {
            Some(proxy) => Upstream::Socks5(proxy),
            None => Upstream::Direct,
        }
    };
    let mut started = sandbox::start(Plan {
        paths,
        mode: Mode::Visible(desktop.clone()),
        start_url: Some(site.start.clone()),
        expose_timezone: cfg.sandbox.expose_timezone,
        limits: None,
        program,
    })?;
    let (open_tx, open_rx) = watch::channel(Some(0));
    let quiet = Arc::new(|_line: String| {});
    let relay =
        relay::start(vec![SiteState::new(site.clone())], open_rx, started.dns, started.listener, upstream, quiet)?;
    // Firefox's console chatter is of no use here.
    drop(started.child.stdout.take());
    drop(started.child.stderr.take());
    reporter.status(format!(
        "A Firefox window for {} is open on {}. Sign in there, then close the window: the in-game browser starts with \
         that session. The window reaches only {} and nothing else; Firefox will not save your password.",
        site.title,
        desktop.describe(),
        site.hosts.iter().map(|h| h.to_string()).collect::<Vec<_>>().join(", ")
    ));
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|e| Error::io("installing a SIGINT handler", e))?;
    let status = tokio::select! {
        status = started.child.wait() => Some(status.map_err(|e| Error::io("waiting for the browser", e))?),
        _ = sigint.recv() => {
            // Ctrl-C ends the sandbox outright. What the site had stored by then stays in
            // the profile; the login is not recorded as completed.
            let _ = started.child.kill().await;
            None
        }
    };
    let _ = open_tx.send(None);
    let used = relay.stats(0).map(|s| s.connections.load(std::sync::atomic::Ordering::Relaxed)).unwrap_or(0);
    relay.shutdown();
    drop(started.lock);
    let Some(status) = status else {
        reporter.status(format!(
            "Stopped with Ctrl-C: the window is gone. What {} had stored in the profile by then is kept, but the \
             login is not recorded as completed; run the command again and close the window when you are done.",
            site.title
        ));
        return Ok(130);
    };
    super::record_login(paths, &site.name)?;
    reporter.status(format!(
        "The browser window was closed ({status}; {used} connection(s) to {} were relayed). The in-game browser \
         starts with this session; its profile is {}.",
        site.title,
        sandbox::profile_dir(paths).display()
    ));
    Ok(0)
}
