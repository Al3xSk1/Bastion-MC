//! The Realms endpoint table, the session cookie and the join answer: pure functions, for the
//! credential broker (`egress::broker`) and for the fuzz targets.
//!
//! The Realms service does not take `Authorization: Bearer`; every request carries the session
//! in a cookie, `sid=token:<access token>:<profile id>;user=<name>;version=<game version>`
//! (`RealmsClient.execute`, the same from 1.20.1 to 26.3). The game holds the placeholder, so
//! that is what its cookie contains. For the requests on the table below the broker rebuilds
//! the cookie from scratch around the real token — checked: exactly one `sid`, the placeholder
//! and nothing else as the token, the session's own profile — and drops every other cookie.
//! The 26.3 "frontend v2" (`java.frontend.realms.minecraft-services.net`) carries the raw token
//! in an `x-mojangauth` header instead; there only the one request a player makes (redeeming an
//! invite code) is taken, with the same check.
//!
//! What the table lets the game do with your account, by family (paths from the clients'
//! `RealmsClient`, 1.20.1 … 26.3): see the Realms list, its notifications, news and invitations;
//! accept or decline an invitation (or redeem an invite code, 26.3); agree to the Realms terms;
//! report region pings; and **join** a Realm — which goes through the approval (`approval`).
//! Everything an owner does to a Realm — configure, switch or reset a world, upload, download or
//! restore backups, invite, op or remove players, open, close, delete, create — is answered
//! `403` here and never leaves the machine: manage your Realm from the official client.

use hyper::Method;

/// The Realms API (every family) and the host 1.21.8+ uses in its place when Realms' feature
/// flag `realms_in_aks` is set.
pub const API_HOSTS: [&str; 2] = ["pc.realms.minecraft.net", "java.frontendlegacy.realms.minecraft-services.net"];
/// The 26.3 "frontend v2" (invite codes).
pub const FRONTEND_V2: &str = "java.frontend.realms.minecraft-services.net";
/// The domains the per-launch certificate authority may issue certificates for, when Realms
/// is brokered (its name constraints).
pub const DOMAINS: [&str; 2] = ["realms.minecraft.net", "realms.minecraft-services.net"];

/// Is this one of the Realms hosts the broker answers for?
pub fn is_realms_host(name: &str) -> bool {
    API_HOSTS.contains(&name) || name == FRONTEND_V2
}

/// What the broker does with one Realms request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Forward with the session cookie rebuilt around the real token.
    Cookie(&'static str),
    /// Forward with `x-mojangauth` set to the real token (frontend v2).
    MojangAuth(&'static str),
    /// The join: `GET /worlds/v1/<world>/join/pc`. Needs the approval.
    Join(u64),
    /// Forward with no credential (a read the table does not know: Realms will refuse it).
    Anonymous,
    Refuse {
        status: u16,
        reason: String,
        security: bool,
    },
}

fn refuse(status: u16, reason: impl Into<String>, security: bool) -> Verdict {
    Verdict::Refuse { status, reason: reason.into(), security }
}

/// A Realm's id: a decimal number.
fn world_id(segment: &str) -> Option<u64> {
    (!segment.is_empty() && segment.len() <= 19 && segment.bytes().all(|b| b.is_ascii_digit()))
        .then(|| segment.parse().ok())
        .flatten()
}

/// An invitation id or an invite code: letters, digits and dashes.
fn token_like(segment: &str) -> bool {
    (1..=64).contains(&segment.len()) && segment.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The decision table for a Realms host. `has_query`: the target had a query string.
pub fn classify(host: &str, method: &Method, path: &str, has_query: bool) -> Verdict {
    if ![Method::GET, Method::POST, Method::PUT, Method::DELETE].contains(method) {
        return refuse(405, format!("method {method} is not relayed"), false);
    }
    if !crate::egress::broker::path_is_canonical(path) {
        return refuse(400, "the request path is not in canonical form", false);
    }
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let known: Option<Verdict> = if host == FRONTEND_V2 {
        match (method.as_str(), segments.as_slice()) {
            ("POST", ["api", "v1.0", "invite", "codes", "use", code]) if token_like(code) => {
                Some(Verdict::MojangAuth("redeem an invite code"))
            }
            _ => None,
        }
    } else if API_HOSTS.contains(&host) {
        match (method.as_str(), segments.as_slice()) {
            ("GET", ["mco", "available"])
            | ("GET", ["mco", "client", "compatible"])
            | ("GET", ["mco", "stageAvailable"])
            | ("GET", ["mco", "v1", "news"])
            | ("GET", ["feature", "v1"])
            | ("GET", ["trial"])
            | ("GET", ["worlds"])
            | ("GET", ["worlds", "listUserWorldsOfType", "any"])
            | ("GET", ["worlds", "listPrereleaseEligibleWorlds"])
            | ("GET", ["invites", "count", "pending"])
            | ("GET", ["invites", "pending"])
            | ("GET", ["activities", "liveplayerlist"])
            | ("GET", ["notifications"])
            | ("GET", ["regions", "preferredRegions"]) => Some(Verdict::Cookie("read the Realms list")),
            ("GET", ["worlds", id]) if world_id(id).is_some() => Some(Verdict::Cookie("read a Realm")),
            ("GET", ["worlds", id, "configuration", "tier"]) if world_id(id).is_some() => {
                Some(Verdict::Cookie("read a Realm"))
            }
            ("GET", ["subscriptions", id]) if world_id(id).is_some() => Some(Verdict::Cookie("read a Realm")),
            ("GET", ["worlds", id, "backups"]) if world_id(id).is_some() => Some(Verdict::Cookie("read a Realm")),
            ("GET", ["worlds", "v1", id, "join", "pc"]) => match world_id(id) {
                Some(world) => Some(Verdict::Join(world)),
                None => Some(refuse(400, "malformed Realm id", false)),
            },
            ("POST", ["notifications", "seen"]) | ("POST", ["notifications", "dismiss"]) => {
                Some(Verdict::Cookie("mark notifications"))
            }
            ("POST", ["mco", "tos", "agreed"]) => Some(Verdict::Cookie("agree to the Realms terms")),
            ("POST", ["regions", "ping", "stat"]) => Some(Verdict::Cookie("report region pings")),
            ("PUT", ["invites", "accept" | "reject", id]) if token_like(id) => {
                Some(Verdict::Cookie("answer an invitation"))
            }
            _ => None,
        }
    } else {
        return refuse(421, "not a Realms host", true);
    };
    match known {
        Some(Verdict::Refuse { .. }) => known.expect("matched"),
        Some(_) if has_query => refuse(400, "credentialed endpoints take no query string", false),
        Some(verdict) => verdict,
        None if *method == Method::GET => Verdict::Anonymous,
        None => refuse(
            403,
            format!(
                "{method} {path} is not something the game may do with your Realms: managing a Realm (its worlds, \
                 backups, players, invitations) is refused here — use the official client for that"
            ),
            true,
        ),
    }
}

/// Why a cookie was not rebuilt.
pub type CookieRefusal = &'static str;

fn cookie_safe(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// The cookie to send in place of the game's: `sid` rebuilt around `token`, `user` and
/// `version` copied if they have the shapes the client gives them, everything else dropped.
pub fn rebuild_cookie(cookie: &[u8], placeholder: &str, token: &[u8], profile: &str) -> Result<Vec<u8>, CookieRefusal> {
    let text = std::str::from_utf8(cookie).map_err(|_| "the cookie is not text")?;
    if text.len() > 4096 {
        return Err("the cookie is too long");
    }
    let (mut sid, mut user, mut version) = (None, None, None);
    for part in text.split(';') {
        let part = part.trim_matches(' ');
        let Some((name, value)) = part.split_once('=') else {
            if part.is_empty() {
                continue;
            }
            return Err("a cookie without a value");
        };
        let slot = match name {
            "sid" => &mut sid,
            "user" => &mut user,
            "version" => &mut version,
            _ => continue, // not relayed
        };
        if slot.replace(value).is_some() {
            return Err("a cookie appears twice");
        }
    }
    let sid = sid.ok_or("no session cookie")?;
    let rest = sid.strip_prefix("token:").ok_or("the session cookie is not a token")?;
    let (presented, uuid) = rest.rsplit_once(':').ok_or("the session cookie is not a token")?;
    if presented != placeholder {
        return Err("the session cookie does not carry the credential the launcher handed out");
    }
    let uuid: String = uuid.chars().filter(|c| *c != '-').collect();
    if !uuid.eq_ignore_ascii_case(profile) {
        return Err("the session cookie is for a different profile than the one signed in");
    }
    if !cookie_safe(token) {
        return Err("the access token cannot be written into a cookie");
    }
    let mut out = Vec::with_capacity(token.len() + 128);
    out.extend_from_slice(b"sid=token:");
    out.extend_from_slice(token);
    out.push(b':');
    out.extend_from_slice(profile.as_bytes());
    if let Some(user) =
        user.filter(|u| (1..=16).contains(&u.len()) && u.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
    {
        out.extend_from_slice(b";user=");
        out.extend_from_slice(user.as_bytes());
    }
    if let Some(version) = version.filter(|v| {
        (1..=64).contains(&v.len())
            && v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b' ' | b'+'))
    }) {
        out.extend_from_slice(b";version=");
        out.extend_from_slice(version.as_bytes());
    }
    Ok(out)
}

/// Where a Realm's server is, as its join answer says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    /// An IP literal or a host name (lower case).
    pub host: String,
    pub port: u16,
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

/// `"host:port"`, `"[v6]:port"`, or a host alone (the default game port).
pub fn parse_address(text: &str) -> Option<Address> {
    if text.is_empty() || text.len() > 260 {
        return None;
    }
    let (host, port) = if let Some(rest) = text.strip_prefix('[') {
        let (host, rest) = rest.split_once(']')?;
        host.parse::<std::net::Ipv6Addr>().ok()?;
        let port = match rest {
            "" => crate::config::DEFAULT_GAME_PORT,
            rest => rest.strip_prefix(':')?.parse().ok()?,
        };
        (host.to_ascii_lowercase(), port)
    } else {
        let (host, port) = match text.rsplit_once(':') {
            Some((host, port)) => (host, port.parse::<u16>().ok()?),
            None => (text, crate::config::DEFAULT_GAME_PORT),
        };
        let host = crate::config::normalize_host(host).ok()?;
        (host, port)
    };
    (port != 0).then_some(Address { host, port })
}

/// The join answer: `{"address": "…", "resourcePackUrl": …, …}`. Returns the server's address
/// and the object, so that the answer handed to the game can be the same object with only the
/// address replaced.
pub fn parse_join_reply(body: &[u8]) -> Result<(Address, serde_json::Map<String, serde_json::Value>), &'static str> {
    if body.len() > 64 * 1024 {
        return Err("the join answer is too large");
    }
    let Ok(serde_json::Value::Object(object)) = serde_json::from_slice::<serde_json::Value>(body) else {
        return Err("the join answer is not a JSON object");
    };
    let address = object
        .get("address")
        .and_then(serde_json::Value::as_str)
        .and_then(parse_address)
        .ok_or("the join answer has no usable server address")?;
    Ok((address, object))
}

/// The join answer the game is given: the same object, with the address of the one-time pass.
pub fn rewrite_join_reply(
    mut object: serde_json::Map<String, serde_json::Value>,
    pass: std::net::SocketAddrV4,
) -> Vec<u8> {
    object.insert("address".into(), serde_json::Value::String(pass.to_string()));
    serde_json::Value::Object(object).to_string().into_bytes()
}

/// A Realms-style error body, which the client shows (`errorMsg`), plus the fields the broker's
/// other replies carry.
pub fn error_body(status: u16, message: &str) -> String {
    serde_json::json!({
        "errorCode": status,
        "errorMsg": format!("bastion-mc: {message}"),
        "error": "bastion-mc",
        "errorMessage": format!("bastion-mc: {message}"),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PC: &str = "pc.realms.minecraft.net";

    fn status(v: &Verdict) -> Option<(u16, bool)> {
        if let Verdict::Refuse { status, security, .. } = v { Some((*status, *security)) } else { None }
    }

    #[test]
    fn the_paths_a_player_uses_are_on_the_table_in_every_family() {
        for host in API_HOSTS {
            for (method, path) in [
                (Method::GET, "/mco/available"),
                (Method::GET, "/mco/client/compatible"),
                (Method::GET, "/mco/stageAvailable"), // 1.20.1
                (Method::GET, "/mco/v1/news"),
                (Method::GET, "/feature/v1"), // 1.21.8+
                (Method::GET, "/worlds"),
                (Method::GET, "/worlds/listUserWorldsOfType/any"), // 1.20.4+
                (Method::GET, "/worlds/listPrereleaseEligibleWorlds"),
                (Method::GET, "/worlds/123456"),
                (Method::GET, "/worlds/123456/configuration/tier"), // 26.3
                (Method::GET, "/invites/count/pending"),            // until 1.21.10
                (Method::GET, "/invites/pending"),
                (Method::GET, "/activities/liveplayerlist"),
                (Method::GET, "/notifications"),
                (Method::GET, "/regions/preferredRegions"),
                (Method::GET, "/trial"),
                (Method::POST, "/notifications/seen"),
                (Method::POST, "/notifications/dismiss"),
                (Method::POST, "/mco/tos/agreed"),
                (Method::POST, "/regions/ping/stat"),
                (Method::PUT, "/invites/accept/a1b2-c3"),
                (Method::PUT, "/invites/reject/42"),
            ] {
                assert!(matches!(classify(host, &method, path, false), Verdict::Cookie(_)), "{host} {method} {path}");
            }
            assert_eq!(classify(host, &Method::GET, "/worlds/v1/987654321/join/pc", false), Verdict::Join(987654321));
        }
        assert!(matches!(
            classify(FRONTEND_V2, &Method::POST, "/api/v1.0/invite/codes/use/AbC-123", false),
            Verdict::MojangAuth(_)
        ));
    }

    #[test]
    fn what_an_owner_does_to_a_realm_is_refused_here() {
        for (method, path) in [
            (Method::DELETE, "/worlds/1"), // delete the Realm
            (Method::POST, "/worlds/1"),   // rename / describe
            (Method::POST, "/worlds/1/reset"),
            (Method::PUT, "/worlds/1/backups"), // restore a backup
            (Method::PUT, "/worlds/1/backups/upload"),
            (Method::PUT, "/worlds/1/slot/2"),
            (Method::POST, "/worlds/1/slot/2"),
            (Method::PUT, "/worlds/1/open"),
            (Method::PUT, "/worlds/1/close"),
            (Method::POST, "/worlds/1/initialize"),
            (Method::POST, "/worlds/1/configuration"),
            (Method::PUT, "/worlds/1/configuration/renderDistance/12"),
            (Method::PUT, "/worlds/minigames/5/1"),
            (Method::POST, "/worlds/1/createPrereleaseRealm"),
            (Method::POST, "/invites/1"), // invite somebody
            (Method::DELETE, "/invites/1/invite/069a79f444e94726a5befca90e38aaf5"),
            (Method::DELETE, "/invites/1"), // leave a Realm
            (Method::POST, "/ops/1/069a79f444e94726a5befca90e38aaf5"),
            (Method::DELETE, "/ops/1/069a79f444e94726a5befca90e38aaf5"),
            (Method::PUT, "/invites/accept/../../worlds/1"),
        ] {
            let v = classify(PC, &method, path, false);
            assert!(matches!(status(&v), Some((400 | 403, _))), "{method} {path}: {v:?}");
        }
        for (method, path) in [
            (Method::GET, "/api/v1.0/invite/codes1"),
            (Method::POST, "/api/v1.0/invite/codes"),
            (Method::POST, "/api/v1.0/invite/codes/update"),
            (Method::DELETE, "/api/v1.0/invite/codes/delete/x"),
        ] {
            let v = classify(FRONTEND_V2, &method, path, false);
            assert!(matches!(v, Verdict::Refuse { .. } | Verdict::Anonymous), "{method} {path}: {v:?}");
            assert!(!matches!(v, Verdict::Cookie(_) | Verdict::MojangAuth(_)), "{method} {path}");
        }
        // Reads the table does not know go out without a credential (and Realms refuses them).
        assert_eq!(classify(PC, &Method::GET, "/worlds/1/slot/1/download", false), Verdict::Anonymous);
        assert_eq!(classify(PC, &Method::GET, "/worlds/templates/NORMAL", true), Verdict::Anonymous);
    }

    #[test]
    fn spellings_queries_and_hosts_that_are_not_exactly_on_the_table_get_no_credential() {
        for path in
            ["/worlds/v1/1/join/pc/", "/worlds//v1/1/join/pc", "/Worlds", "/worlds/v1/1/join/PC", "/mco/available;x"]
        {
            let v = classify(PC, &Method::GET, path, false);
            assert!(!matches!(v, Verdict::Cookie(_) | Verdict::Join(_)), "{path}: {v:?}");
        }
        assert_eq!(status(&classify(PC, &Method::GET, "/worlds", true)), Some((400, false)));
        assert_eq!(status(&classify(PC, &Method::GET, "/worlds/v1/1/join/pc", true)), Some((400, false)));
        assert_eq!(status(&classify(PC, &Method::GET, "/worlds/v1/x/join/pc", false)), Some((400, false)));
        assert_eq!(
            status(&classify(PC, &Method::GET, "/worlds/v1/99999999999999999999/join/pc", false)),
            Some((400, false))
        );
        assert_eq!(status(&classify("api.minecraftservices.com", &Method::GET, "/worlds", false)), Some((421, true)));
        assert_eq!(status(&classify(PC, &Method::PATCH, "/worlds", false)), Some((405, false)));
        assert!(is_realms_host(PC) && is_realms_host(FRONTEND_V2) && !is_realms_host("realms.minecraft.net"));
    }

    #[test]
    fn the_cookie_is_rebuilt_around_the_real_token_and_nothing_else_is_relayed() {
        let placeholder = "bastion-placeholder-x";
        let profile = "069a79f444e94726a5befca90e38aaf5";
        let token = b"eyJ.REAL.TOKEN";
        let game = format!("sid=token:{placeholder}:{profile};user=Steve;version=26.3");
        assert_eq!(
            rebuild_cookie(game.as_bytes(), placeholder, token, profile).unwrap(),
            format!("sid=token:eyJ.REAL.TOKEN:{profile};user=Steve;version=26.3").as_bytes()
        );
        // Spaces after the separators, a dashed profile id, extra cookies: tolerated, extras dropped.
        let spaced = format!(
            "sid=token:{placeholder}:069a79f4-44e9-4726-a5be-fca90e38aaf5; user=Steve; tracking=1; version=1.20.1"
        );
        assert_eq!(
            rebuild_cookie(spaced.as_bytes(), placeholder, token, profile).unwrap(),
            format!("sid=token:eyJ.REAL.TOKEN:{profile};user=Steve;version=1.20.1").as_bytes()
        );
        // A user or version of another shape is left out rather than relayed.
        let odd = format!("sid=token:{placeholder}:{profile};user=St\u{e9}ve;version=1.2\x01");
        assert_eq!(
            rebuild_cookie(odd.as_bytes(), placeholder, token, profile).unwrap(),
            format!("sid=token:eyJ.REAL.TOKEN:{profile}").as_bytes()
        );
        for (bad, why) in [
            ("user=Steve;version=26.3".to_string(), "no session cookie"),
            (format!("sid=token:another-token:{profile}"), "does not carry"),
            (format!("sid=token:{placeholder}:00000000000000000000000000000000"), "different profile"),
            (format!("sid={placeholder}"), "not a token"),
            (format!("sid=token:{placeholder}:{profile};sid=token:{placeholder}:{profile}"), "twice"),
            (format!("sid=token:{placeholder}:{profile};garbage"), "without a value"),
        ] {
            let err = rebuild_cookie(bad.as_bytes(), placeholder, token, profile).unwrap_err();
            assert!(err.contains(why), "{bad}: {err}");
        }
        assert!(
            rebuild_cookie(game.as_bytes(), placeholder, b"tok;en=1", profile).is_err(),
            "a token that would break the cookie"
        );
        assert!(rebuild_cookie(b"\xff", placeholder, token, profile).is_err());
    }

    #[test]
    fn the_join_answer_is_read_and_only_its_address_is_replaced() {
        let body = br#"{"address":"203.0.113.9:31234","resourcePackUrl":null,"resourcePackHash":null,"sessionRegionData":{"regionName":"EU"}}"#;
        let (address, object) = parse_join_reply(body).unwrap();
        assert_eq!(address, Address { host: "203.0.113.9".into(), port: 31234 });
        let pass = std::net::SocketAddrV4::new(std::net::Ipv4Addr::new(127, 127, 255, 254), 25565);
        let rewritten: serde_json::Value = serde_json::from_slice(&rewrite_join_reply(object, pass)).unwrap();
        assert_eq!(rewritten["address"], "127.127.255.254:25565");
        assert_eq!(rewritten["sessionRegionData"]["regionName"], "EU");
        assert!(rewritten["resourcePackUrl"].is_null());
        for (text, expected) in [
            ("play.example.org:25570", Some(("play.example.org", 25570))),
            ("Realm.Example.org", Some(("realm.example.org", 25565))),
            ("[2001:db8::1]:25566", Some(("2001:db8::1", 25566))),
            ("203.0.113.9:0", None),
            ("203.0.113.9:70000", None),
            ("", None),
            ("a b:1", None),
            ("[::1", None),
        ] {
            assert_eq!(
                parse_address(text).map(|a| (a.host.clone(), a.port)),
                expected.map(|(h, p)| (h.to_string(), p)),
                "{text}"
            );
        }
        for bad in [&br#"["203.0.113.9:1"]"#[..], br#"{"address":42}"#, br#"{"addr":"203.0.113.9:1"}"#, b"", b"{"] {
            assert!(parse_join_reply(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
        }
    }
}
