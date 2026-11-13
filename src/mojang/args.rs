//! Argument template expansion and the launch frame.
//!
//! Minecraft's version JSON describes arguments as templates (`--accessToken
//! ${auth_access_token}`). Two kinds of variables exist here:
//!
//! * plain values — paths, version names;
//! * secret values — the access token (and the legacy `auth_session` built from it).
//!
//! Secret values are only ever expanded *directly into the mlock'd launch frame*
//! ([`build_frame`]). [`render_plain`], which produces JVM command-line arguments, refuses
//! any template that mentions a secret variable, so a malformed or malicious version file
//! cannot trick the launcher into putting the token on a command line.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    error::{Error, Result},
    secret::Secret,
};

/// Must match `BastionBootstrap.java`.
pub const FRAME_MAGIC: u32 = 0x4253_544E; // "BSTN"
pub const FRAME_VERSION: u32 = 1;

pub enum Piece<'a> {
    Lit(String),
    Sec(&'a Secret),
}

#[derive(Default)]
pub struct Vars<'a> {
    plain: BTreeMap<&'static str, String>,
    secret: BTreeMap<&'static str, Vec<Piece<'a>>>,
}

impl<'a> Vars<'a> {
    pub fn set(&mut self, name: &'static str, value: impl Into<String>) -> &mut Self {
        self.plain.insert(name, value.into());
        self
    }

    pub fn set_secret(&mut self, name: &'static str, pieces: Vec<Piece<'a>>) -> &mut Self {
        self.secret.insert(name, pieces);
        self
    }

    pub fn is_secret(&self, name: &str) -> bool {
        self.secret.contains_key(name)
    }
}

enum Token<'t> {
    Text(&'t str),
    Var(&'t str),
}

/// Split a template into literal text and `${name}` references. A `${` without a closing
/// brace, or with characters outside `[A-Za-z0-9_]`, is literal text.
fn tokenize(template: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end)
                if !after[..end].is_empty() && after[..end].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') =>
            {
                if start > 0 {
                    out.push(Token::Text(&rest[..start]));
                }
                out.push(Token::Var(&after[..end]));
                rest = &after[end + 1..];
            }
            _ => {
                out.push(Token::Text(&rest[..start + 2]));
                rest = after;
            }
        }
    }
    if !rest.is_empty() {
        out.push(Token::Text(rest));
    }
    out
}

/// Expand a template that must not involve secrets (JVM arguments, paths).
/// Unknown variables expand to nothing and are reported through `unknown`.
pub fn render_plain(template: &str, vars: &Vars<'_>, unknown: &mut BTreeSet<String>) -> Result<String> {
    let mut out = String::with_capacity(template.len());
    for tok in tokenize(template) {
        match tok {
            Token::Text(t) => out.push_str(t),
            Token::Var(name) if vars.is_secret(name) => {
                return Err(Error::Policy(format!(
                    "refusing to expand secret variable ${{{name}}} outside the launch frame (template {template:?})"
                )));
            }
            Token::Var(name) => match vars.plain.get(name) {
                Some(v) => out.push_str(v),
                None => {
                    unknown.insert(name.to_string());
                }
            },
        }
    }
    Ok(out)
}

/// For logs: same expansion with every secret replaced by a marker.
pub fn render_redacted(template: &str, vars: &Vars<'_>) -> String {
    let mut out = String::new();
    for tok in tokenize(template) {
        match tok {
            Token::Text(t) => out.push_str(t),
            Token::Var(name) if vars.is_secret(name) => out.push_str("<redacted>"),
            Token::Var(name) => out.push_str(vars.plain.get(name).map(String::as_str).unwrap_or("")),
        }
    }
    out
}

fn rendered_len(template: &str, vars: &Vars<'_>) -> usize {
    tokenize(template)
        .iter()
        .map(|tok| match tok {
            Token::Text(t) => t.len(),
            Token::Var(name) => match vars.secret.get(name) {
                Some(pieces) => pieces
                    .iter()
                    .map(|p| match p {
                        Piece::Lit(s) => s.len(),
                        Piece::Sec(s) => s.len(),
                    })
                    .sum(),
                None => vars.plain.get(name).map(String::len).unwrap_or(0),
            },
        })
        .sum()
}

fn render_into(template: &str, vars: &Vars<'_>, out: &mut Secret) -> Result<()> {
    for tok in tokenize(template) {
        match tok {
            Token::Text(t) => out.push(t.as_bytes())?,
            Token::Var(name) => match vars.secret.get(name) {
                Some(pieces) => {
                    for p in pieces {
                        match p {
                            Piece::Lit(s) => out.push(s.as_bytes())?,
                            Piece::Sec(s) => out.push(s.expose())?,
                        }
                    }
                }
                None => out.push(vars.plain.get(name).map(String::as_bytes).unwrap_or(b""))?,
            },
        }
    }
    Ok(())
}

/// Build the frame the bootstrap reads from fd 0: main class followed by every program
/// argument, with secrets expanded in place. The result lives in locked memory and is sized
/// exactly, so the token is copied once and never reallocated.
pub fn build_frame(main_class: &str, arg_templates: &[String], vars: &Vars<'_>) -> Result<Secret> {
    let count = u32::try_from(arg_templates.len() + 1).map_err(|_| Error::Other("too many arguments".into()))?;
    let lens: Vec<usize> = arg_templates.iter().map(|t| rendered_len(t, vars)).collect();
    let total = 12 + 4 + main_class.len() + lens.iter().map(|l| 4 + l).sum::<usize>();
    let mut frame = Secret::with_capacity(total)?;
    frame.push(&FRAME_MAGIC.to_be_bytes())?;
    frame.push(&FRAME_VERSION.to_be_bytes())?;
    frame.push(&count.to_be_bytes())?;
    frame.push(&(main_class.len() as u32).to_be_bytes())?;
    frame.push(main_class.as_bytes())?;
    for (template, len) in arg_templates.iter().zip(&lens) {
        let len32 = u32::try_from(*len).map_err(|_| Error::Other("argument too long".into()))?;
        frame.push(&len32.to_be_bytes())?;
        let before = frame.len();
        render_into(template, vars, &mut frame)?;
        debug_assert_eq!(frame.len() - before, *len);
    }
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(frame: &[u8]) -> Vec<String> {
        let u32_at = |i: usize| u32::from_be_bytes(frame[i..i + 4].try_into().unwrap()) as usize;
        assert_eq!(u32_at(0) as u32, FRAME_MAGIC);
        assert_eq!(u32_at(4) as u32, FRAME_VERSION);
        let count = u32_at(8);
        let mut pos = 12;
        let mut out = Vec::new();
        for _ in 0..count {
            let len = u32_at(pos);
            pos += 4;
            out.push(String::from_utf8(frame[pos..pos + len].to_vec()).unwrap());
            pos += len;
        }
        assert_eq!(pos, frame.len(), "no trailing bytes");
        out
    }

    #[test]
    fn frame_carries_token_and_legacy_session() {
        let token = Secret::from_slice(b"eyJ.SECRET.jwt").unwrap();
        let mut vars = Vars::default();
        vars.set("auth_player_name", "Steve").set("game_directory", "/game");
        vars.set_secret("auth_access_token", vec![Piece::Sec(&token)]);
        vars.set_secret(
            "auth_session",
            vec![Piece::Lit("token:".into()), Piece::Sec(&token), Piece::Lit(":abcd".into())],
        );
        let templates: Vec<String> = [
            "--username",
            "${auth_player_name}",
            "--accessToken",
            "${auth_access_token}",
            "${auth_session}",
            "--gameDir",
            "${game_directory}",
            "x${nope}y",
            "${",
            "a${b c}",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let frame = build_frame("net.minecraft.client.main.Main", &templates, &vars).unwrap();
        let got = decode(frame.expose());
        assert_eq!(
            got,
            vec![
                "net.minecraft.client.main.Main",
                "--username",
                "Steve",
                "--accessToken",
                "eyJ.SECRET.jwt",
                "token:eyJ.SECRET.jwt:abcd",
                "--gameDir",
                "/game",
                "xy",
                "${",
                "a${b c}"
            ]
        );
    }

    #[test]
    fn secrets_cannot_reach_command_line_arguments() {
        let token = Secret::from_slice(b"SECRET").unwrap();
        let mut vars = Vars::default();
        vars.set("classpath", "/a:/b");
        vars.set_secret("auth_access_token", vec![Piece::Sec(&token)]);
        let mut unknown = BTreeSet::new();
        assert_eq!(render_plain("-cp=${classpath}", &vars, &mut unknown).unwrap(), "-cp=/a:/b");
        assert!(render_plain("-Dtoken=${auth_access_token}", &vars, &mut unknown).is_err());
        assert_eq!(render_plain("-Dx=${mystery}", &vars, &mut unknown).unwrap(), "-Dx=");
        assert!(unknown.contains("mystery"));
        let shown = render_redacted("--accessToken=${auth_access_token}", &vars);
        assert!(!shown.contains("SECRET") && shown.contains("<redacted>"));
    }
}
