//! What the bridge accepts: the sign-in page the game may have shown, and the address that
//! may go back to it.
//!
//! Both checks parse exactly one shape each and refuse everything else. They are written for
//! a hostile game: the start address arrives from the sandbox, and the one thing the game
//! gains from the bridge is a page the user trusts, shown by the launcher. So the page must be
//! Microsoft's sign-in for **this one client**, with **this one scope**, sending the code to
//! **this one redirect**, protected by **PKCE with S256**:
//!
//! * a different `client_id` or `scope` could make the user approve some other application's
//!   access (mail, files, the account itself) while believing they sign in to Minecraft;
//! * a different `redirect_uri` would send the code somewhere other than back to the game;
//! * without S256 the code in the final address would be usable by anyone who saw it.
//!
//! Parser differentials are the other danger: the launcher must read the same parameters as
//! login.live.com does. So names are plain words (no percent-escapes, which a server would
//! decode), compared without regard to case and never repeated (a server whose parameter
//! names ignore case sees `CLIENT_ID=…&client_id=…` as one name twice), and `;` — a separator
//! for some servers — does not appear at all.
//!
//! The final address comes from trusted input (the user pasted it), but it is checked as
//! strictly: it must be under [`REDIRECT_URI`], carry a code, and carry this sign-in's state.
//! What goes to the game is then not the pasted text but an address rebuilt from those two
//! values, in the form login.live.com sends — so nothing else the paste held reaches the game.
//!
//! **Not verified against Microsoft** (there is no account here): the parameters the real
//! page carries. The start address is made by Xbox's sign-in service (SISU), not by the game,
//! and may hold parameters this does not know; those are allowed when they cannot change
//! where the code goes or what it grants. The one check most likely to need care after the
//! first real sign-in is [`SCOPE`]: a page that asks for more is refused, by design, with the
//! scope named in the refusal.

use super::{CLIENT_ID, REDIRECT_URI, START_PREFIX};
use crate::util;

/// The only scope the bridge lets the game ask for: Xbox user authentication, which is what
/// the game's sign-in library exchanges the code for (it is the scope of every public
/// implementation of this client's sign-in, e.g. gophertunnel's).
pub const SCOPE: &str = "service::user.auth.xboxlive.com::MBI_SSL";

/// The longest start address. The front end opens at most 4096 bytes (its link check); the
/// pages Xbox's sign-in service makes are expected to be well below (not measured).
pub const MAX_START: usize = 4096;
/// The longest address that can go back: `mcpelauncher-client` reads the webview's output
/// with one 1024-byte read, and the newline is part of it.
pub const MAX_FINAL: usize = 1023;
/// Parameters in a start address, at most.
const MAX_PARAMS: usize = 64;
/// The longest state, decoded. The game makes it; real ones are a few dozen bytes.
const MAX_STATE: usize = 512;
/// The longest code. Microsoft's are about 50 bytes; the address as a whole must fit
/// [`MAX_FINAL`] anyway.
const MAX_CODE: usize = 900;

/// Why a request was refused, in words the log and the game's error may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refusal(pub &'static str);

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// A request that passed: the page to show, and what the address it ends on must carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// The start address, exactly as the game sent it.
    pub url: String,
    /// The state, decoded.
    state: Vec<u8>,
}

impl Checked {
    #[doc(hidden)]
    pub fn for_test(state: &str) -> Checked {
        Checked { url: format!("{START_PREFIX}state={state}"), state: state.as_bytes().to_vec() }
    }
}

/// Parameters a start address must not carry, whatever their value: ways for a server to take
/// the request's parameters from somewhere other than the query the launcher checked (a
/// request object, `request` / `request_uri`, may override `client_id`, `scope` and
/// `redirect_uri`), to ask for a token for another resource, or to name another address to
/// return to (the parameters of Microsoft's older sign-in endpoints). Whether login.live.com
/// honours any of them for this endpoint is not known; none has a reason to be there.
const REFUSED_PARAMS: &[&str] = &[
    "request",
    "request_uri",
    "resource",
    "ru",
    "wreply",
    "wa",
    "wctx",
    "return_url",
    "returnurl",
    "redirect_url",
    "redirecturl",
    "post_logout_redirect_uri",
];

/// Characters a parameter value may hold, besides `%XX` escapes: what an encoder leaves
/// as it is in a query, without `&` (the separator), `;` (a separator elsewhere) and `#`.
fn value_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._~!$'()*+,:@/?=".contains(&b)
}

fn name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._".contains(&b)
}

/// Decode `%XX` escapes and `+` (a space, in a query). `None` for a broken escape.
fn decode(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes.get(i + 1..i + 3)?;
                if !hex.iter().all(u8::is_ascii_hexdigit) {
                    return None;
                }
                out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    Some(out)
}

/// One parameter: its name in lower case, and its value decoded.
struct Param {
    name: String,
    value: Vec<u8>,
}

/// Split a query into parameters, refusing anything a server might read differently.
fn params(query: &str, max: usize) -> Result<Vec<Param>, &'static str> {
    if query.contains(';') {
        return Err("the address holds a `;`, which some servers read as a separator");
    }
    let mut out: Vec<Param> = Vec::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        if out.len() == max {
            return Err("the address holds too many parameters");
        }
        let (name, raw) = pair.split_once('=').unwrap_or((pair, ""));
        if name.is_empty() || name.len() > 64 || !name.bytes().all(name_byte) {
            return Err("the address holds a parameter name that is not a plain word");
        }
        if !raw.bytes().all(|b| value_byte(b) || b == b'%') {
            return Err("the address holds characters a parameter cannot hold");
        }
        let value = decode(raw).ok_or("the address holds a broken %-escape")?;
        let name = name.to_ascii_lowercase();
        if out.iter().any(|p| p.name == name) {
            return Err("the address names a parameter twice");
        }
        out.push(Param { name, value });
    }
    Ok(out)
}

fn find<'p>(params: &'p [Param], name: &str) -> Option<&'p Param> {
    params.iter().find(|p| p.name == name)
}

/// Check a request from the sandbox: the page the game wants shown (`start`) and the prefix
/// of the address it waits for (`end`).
pub fn check_request(start: &str, end: &str) -> Result<Checked, Refusal> {
    if end != REDIRECT_URI {
        return Err(Refusal("the end of the sign-in is not Minecraft's own redirect (ms-xal-0000000048183522://auth)"));
    }
    if start.len() > MAX_START {
        return Err(Refusal("the sign-in page's address is longer than 4096 bytes"));
    }
    if !start.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(Refusal("the sign-in page's address holds something other than printable ASCII"));
    }
    let Some(query) = start.strip_prefix(START_PREFIX) else {
        return Err(Refusal("the sign-in page is not https://login.live.com/oauth20_authorize.srf"));
    };
    if query.contains('#') {
        return Err(Refusal("the sign-in page's address has a fragment"));
    }
    let params = params(query, MAX_PARAMS).map_err(Refusal)?;
    if params.iter().any(|p| REFUSED_PARAMS.contains(&p.name.as_str())) {
        return Err(Refusal(
            "the sign-in page's address carries a parameter that could override the checked ones or name another \
             return address",
        ));
    }
    let exactly = |name: &str, want: &str, refusal: &'static str| -> Result<(), Refusal> {
        match find(&params, name) {
            Some(p) if p.value == want.as_bytes() => Ok(()),
            _ => Err(Refusal(refusal)),
        }
    };
    exactly("client_id", CLIENT_ID, "the sign-in is not for Minecraft's client (client_id 0000000048183522)")?;
    exactly("redirect_uri", REDIRECT_URI, "the sign-in does not return to ms-xal-0000000048183522://auth")?;
    exactly("response_type", "code", "the sign-in does not ask for an authorization code (response_type)")?;
    exactly(
        "scope",
        SCOPE,
        "the sign-in asks for a scope other than Xbox sign-in (service::user.auth.xboxlive.com::MBI_SSL)",
    )?;
    exactly("code_challenge_method", "S256", "the sign-in is not protected by PKCE with S256")?;
    match find(&params, "code_challenge") {
        Some(p)
            if (43..=128).contains(&p.value.len())
                && p.value.iter().all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(b)) => {}
        _ => return Err(Refusal("the sign-in's PKCE challenge is missing or malformed")),
    }
    // How the code comes back: in the query, the only form the final address is checked for.
    if let Some(p) = find(&params, "response_mode")
        && p.value != b"query"
    {
        return Err(Refusal("the sign-in would return the code in another form than the query (response_mode)"));
    }
    let state = match find(&params, "state") {
        Some(p) if !p.value.is_empty() && p.value.len() <= MAX_STATE && p.value.iter().all(u8::is_ascii_graphic) => {
            p.value.clone()
        }
        _ => return Err(Refusal("the sign-in's state is missing or malformed")),
    };
    Ok(Checked { url: start.to_string(), state })
}

/// Check the address a sign-in ended on, as the user gave it, against the request it answers.
/// Returns the address for the game: rebuilt from the code and the state alone.
pub fn check_final(answer: &str, request: &Checked) -> Result<String, &'static str> {
    let Some(query) = answer.strip_prefix(REDIRECT_URI).and_then(|rest| rest.strip_prefix('?')) else {
        return Err(
            "it does not start with ms-xal-0000000048183522://auth? (copy the whole address the browser ended on)",
        );
    };
    let query = query.split_once('#').map_or(query, |(q, _)| q);
    if query.bytes().any(|b| b == b' ') {
        // A space only ever appears in a description Microsoft did not encode; such an
        // address is an error report, never one with a code.
        if !query.split('&').any(|p| p.starts_with("error=")) {
            return Err("it holds a space; copy the address again, whole");
        }
    }
    let mut code: Option<&str> = None;
    let mut state: Option<&str> = None;
    let mut error: Option<&str> = None;
    for pair in query.split('&') {
        let (name, raw) = pair.split_once('=').unwrap_or((pair, ""));
        let slot = match name {
            "code" => &mut code,
            "state" => &mut state,
            "error" => &mut error,
            _ => continue,
        };
        if slot.replace(raw).is_some() {
            return Err("it names the code, the state or an error twice");
        }
    }
    if let Some(error) = error {
        let word: String = error.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(64).collect();
        return Err(match word.as_str() {
            "access_denied" => "the sign-in page reports that the sign-in was declined (access_denied)",
            "" => "the sign-in page reports an error",
            _ => "the sign-in page reports an error (the address holds `error=` and no code)",
        });
    }
    let Some(code) = code.filter(|c| !c.is_empty()) else {
        return Err("it holds no code (did the sign-in finish?)");
    };
    if code.len() > MAX_CODE || !code.bytes().all(|b| value_byte(b) || b == b'%') || decode(code).is_none() {
        return Err("its code holds characters a code cannot hold");
    }
    let Some(state) = state else { return Err("it holds no state") };
    if !state.bytes().all(|b| value_byte(b) || b == b'%') {
        return Err("its state holds characters a state cannot hold");
    }
    match decode(state) {
        Some(decoded) if util::ct_eq(&decoded, &request.state) => {}
        _ => return Err("its state is not this sign-in's (is it from an earlier sign-in?)"),
    }
    let rebuilt = format!("{REDIRECT_URI}?code={code}&state={state}");
    if rebuilt.len() > MAX_FINAL {
        return Err("it is longer than mcpelauncher-client can read (1023 characters)");
    }
    Ok(rebuilt)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

    /// A start address in the shape Xbox's sign-in service is expected to make (the
    /// parameters of the authorize request, plus page options it adds).
    fn start(extra: &str) -> String {
        format!(
            "{START_PREFIX}lw=1&fl=dob,easi2&xsup=1&code_challenge={CHALLENGE}&code_challenge_method=S256\
             &state=Zm9vYmFy-_.~&client_id=0000000048183522&response_type=code\
             &scope=service%3A%3Auser.auth.xboxlive.com%3A%3AMBI_SSL\
             &redirect_uri=ms-xal-0000000048183522%3A%2F%2Fauth&display=android_phone&prompt=select_account{extra}"
        )
    }

    fn ok(extra: &str) -> Checked {
        check_request(&start(extra), REDIRECT_URI).unwrap_or_else(|e| panic!("{extra:?}: {e}"))
    }

    fn refused(url: &str) -> &'static str {
        match check_request(url, REDIRECT_URI) {
            Err(Refusal(why)) => why,
            Ok(_) => panic!("accepted: {url}"),
        }
    }

    #[test]
    fn microsofts_page_for_minecrafts_client_is_accepted_with_its_state() {
        let checked = ok("");
        assert_eq!(checked.url, start(""));
        assert_eq!(checked.state, b"Zm9vYmFy-_.~");
        // Unencoded forms decode to the same values.
        let plain = start("")
            .replace("service%3A%3Auser.auth.xboxlive.com%3A%3AMBI_SSL", SCOPE)
            .replace("ms-xal-0000000048183522%3A%2F%2Fauth", "ms-xal-0000000048183522://auth");
        assert!(check_request(&plain, REDIRECT_URI).is_ok());
        // Options of the page that change nothing about where the code goes are allowed.
        ok("&mkt=de-DE&uaid=0123abcd&response_mode=query&login_hint=someone%40example.org");
    }

    #[test]
    fn another_host_scheme_path_or_end_is_refused() {
        let good = start("");
        for url in [
            good.replace("https://", "http://"),
            good.replace("login.live.com", "login.live.com.evil.example"),
            good.replace("login.live.com", "login.live.com:443"),
            good.replace("login.live.com", "user@login.live.com"),
            good.replace("login.live.com", "LOGIN.live.com"),
            good.replace("/oauth20_authorize.srf", "/oauth20_authorize.srf/"),
            good.replace("/oauth20_authorize.srf", "/ppsecure/post.srf"),
            good.replace("https://login.live.com", "https://login.microsoftonline.com"),
            format!("{good}#fragment"),
        ] {
            refused(&url);
        }
        assert!(check_request(&good, "ms-xal-0000000048183522://auth/").is_err());
        assert!(check_request(&good, "ms-xal-000000004c17c01a://auth").is_err());
        assert!(check_request(&good, "").is_err());
    }

    #[test]
    fn another_client_scope_redirect_or_no_pkce_is_refused() {
        let good = start("");
        assert!(refused(&good.replace("client_id=0000000048183522", "client_id=00000000402b5328")).contains("client"));
        assert!(refused(&good.replace("&client_id=0000000048183522", "")).contains("client"));
        assert!(
            refused(&good.replace(
                "scope=service%3A%3Auser.auth.xboxlive.com%3A%3AMBI_SSL",
                "scope=service%3A%3Aoutlook.office.com%3A%3AMBI_SSL"
            ))
            .contains("scope")
        );
        assert!(
            refused(&good.replace("MBI_SSL&", "MBI_SSL+offline_access&")).contains("scope"),
            "more than the one scope"
        );
        assert!(
            refused(&good.replace("&scope=service%3A%3Auser.auth.xboxlive.com%3A%3AMBI_SSL", "")).contains("scope")
        );
        assert!(
            refused(&good.replace("ms-xal-0000000048183522%3A%2F%2Fauth", "https%3A%2F%2Fevil.example%2F"))
                .contains("return")
        );
        assert!(refused(&good.replace("response_type=code", "response_type=token")).contains("response_type"));
        assert!(refused(&good.replace("response_type=code", "response_type=code+id_token")).contains("response_type"));
        assert!(refused(&good.replace("code_challenge_method=S256", "code_challenge_method=plain")).contains("PKCE"));
        assert!(refused(&good.replace("&code_challenge_method=S256", "")).contains("PKCE"));
        assert!(refused(&good.replace(CHALLENGE, "short")).contains("challenge"));
        assert!(refused(&good.replace(CHALLENGE, &format!("{CHALLENGE}%2B"))).contains("challenge"));
        assert!(refused(&start("&response_mode=fragment")).contains("response_mode"));
        assert!(refused(&start("&response_mode=form_post")).contains("response_mode"));
        assert!(refused(&good.replace("state=Zm9vYmFy-_.~", "state=")).contains("state"));
        assert!(refused(&good.replace("state=Zm9vYmFy-_.~", "state=%00")).contains("state"));
        assert!(refused(&good.replace("state=Zm9vYmFy-_.~", &format!("state={}", "s".repeat(513)))).contains("state"));
    }

    #[test]
    fn parameters_a_server_could_read_differently_are_refused() {
        let good = start("");
        // A second client_id, in any case: which one would Microsoft use?
        assert!(refused(&format!("{good}&client_id=00000000402b5328")).contains("twice"));
        assert!(refused(&format!("{good}&CLIENT_ID=00000000402b5328")).contains("twice"));
        assert!(refused(&format!("{START_PREFIX}Client_Id=x&{}", &good[START_PREFIX.len()..])).contains("twice"));
        // A name a server would decode into `client_id`.
        assert!(refused(&format!("{good}&client%5Fid=00000000402b5328")).contains("plain word"));
        // `;` splits parameters for some servers.
        assert!(refused(&format!("{good}&x=1;client_id=00000000402b5328")).contains(';'));
        assert!(refused(&format!("{good}&x=%G0")).contains("escape"));
        assert!(refused(&format!("{good}&x=%4")).contains("escape"));
        assert!(refused(&format!("{good}&x=a\"b")).contains("characters"));
        assert!(refused(&format!("{good}&x=<script>")).contains("characters"));
        assert!(refused(&format!("{good}&=x")).contains("plain word"));
        assert!(refused(&format!("{good} &x=1")).contains("printable"));
        assert!(refused(&format!("{good}&x=\u{e9}")).contains("printable"));
        assert!(refused(&format!("{good}&x=a\nb")).contains("printable"));
        let many: String = (0..MAX_PARAMS).map(|i| format!("&p{i}=1")).collect();
        assert!(refused(&format!("{good}{many}")).contains("too many"));
        let long = format!("{good}&pad={}", "a".repeat(MAX_START - good.len()));
        assert!(refused(&long).contains("4096"));
        // Empty pieces between separators change nothing.
        ok("&&");
    }

    #[test]
    fn parameters_that_could_override_the_checked_ones_or_name_another_return_are_refused() {
        for name in REFUSED_PARAMS.iter().chain(&["REQUEST_URI", "Ru"]) {
            let url = start(&format!("&{name}=https%3A%2F%2Fevil.example%2F"));
            assert!(refused(&url).contains("override"), "{name}");
        }
        // Names that merely contain one of them are other parameters.
        ok("&rurl=1&wav=2&requested=3");
    }

    #[test]
    fn the_final_address_is_checked_and_rebuilt_from_code_and_state() {
        let request = ok("");
        let code = "M.C507_BL2.2.U.0e5b2bbb-1b34-4e1b-9d29-2f7c0a4f9d4b";
        let pasted = format!("{REDIRECT_URI}?code={code}&state=Zm9vYmFy-_.~&lc=1033");
        let rebuilt = check_final(&pasted, &request).unwrap();
        assert_eq!(rebuilt, format!("{REDIRECT_URI}?code={code}&state=Zm9vYmFy-_.~"));
        // The order of the parameters, an encoded state and a fragment do not matter.
        let other = format!("{REDIRECT_URI}?state=Zm9vYmFy%2D_.~&code={code}#");
        assert_eq!(check_final(&other, &request).unwrap(), format!("{REDIRECT_URI}?code={code}&state=Zm9vYmFy%2D_.~"));
        // What comes back passes the same check again, unchanged.
        assert_eq!(check_final(&rebuilt, &request).unwrap(), rebuilt);
    }

    #[test]
    fn a_wrong_or_incomplete_final_address_is_refused_with_the_reason() {
        let request = ok("");
        let code = "M.C507_BL2.2.U.0e5b2bbb";
        let refused = |answer: String| check_final(&answer, &request).unwrap_err();
        assert!(refused(format!("https://login.live.com/x?code={code}&state=Zm9vYmFy-_.~")).contains("start"));
        assert!(refused(format!("ms-xal-0000000048183522://auth/?code={code}&state=Zm9vYmFy-_.~")).contains("start"));
        assert!(refused(format!("MS-XAL-0000000048183522://auth?code={code}&state=Zm9vYmFy-_.~")).contains("start"));
        assert!(refused(format!("{REDIRECT_URI}?state=Zm9vYmFy-_.~")).contains("no code"));
        assert!(refused(format!("{REDIRECT_URI}?code=&state=Zm9vYmFy-_.~")).contains("no code"));
        assert!(refused(format!("{REDIRECT_URI}?code={code}")).contains("no state"));
        assert!(refused(format!("{REDIRECT_URI}?code={code}&state=other")).contains("earlier sign-in"));
        assert!(refused(format!("{REDIRECT_URI}?code={code}&state=Zm9vYmFy-_.~X")).contains("earlier sign-in"));
        assert!(
            refused(format!("{REDIRECT_URI}?code={code}&code=M.other&state=Zm9vYmFy-_.~")).contains("twice"),
            "two codes"
        );
        assert!(refused(format!("{REDIRECT_URI}?code=a\"b&state=Zm9vYmFy-_.~")).contains("code holds"));
        assert!(refused(format!("{REDIRECT_URI}?code=a%zz&state=Zm9vYmFy-_.~")).contains("code holds"));
        assert!(refused(format!("{REDIRECT_URI}?code=a;b&state=Zm9vYmFy-_.~")).contains("code holds"));
        assert!(refused(format!("{REDIRECT_URI}?code={code} x&state=Zm9vYmFy-_.~")).contains("space"));
        assert!(
            refused(format!(
                "{REDIRECT_URI}?error=access_denied&error_description=The user has denied access&state=Zm9vYmFy-_.~"
            ))
            .contains("declined")
        );
        assert!(refused(format!("{REDIRECT_URI}?error=server_error&state=Zm9vYmFy-_.~")).contains("error"));
        let longer = format!("{REDIRECT_URI}?code={}&state=Zm9vYmFy-_.~", "c".repeat(MAX_CODE + 1));
        assert!(refused(longer).contains("code holds"));
        // Code and state that are each within bounds, but together more than the client reads.
        let state = "s".repeat(300);
        let long_state = check_request(&start("").replace("Zm9vYmFy-_.~", &state), REDIRECT_URI).unwrap();
        let overhead = REDIRECT_URI.len() + "?code=&state=".len() + state.len();
        let fits = format!("{REDIRECT_URI}?code={}&state={state}", "c".repeat(MAX_FINAL - overhead));
        assert_eq!(check_final(&fits, &long_state).unwrap().len(), MAX_FINAL);
        let over = format!("{REDIRECT_URI}?code={}&state={state}", "c".repeat(MAX_FINAL - overhead + 1));
        assert!(check_final(&over, &long_state).unwrap_err().contains("1023"));
    }
}
