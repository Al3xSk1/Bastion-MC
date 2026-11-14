//! The bridge's frames: a `u32` length, big-endian, then that many bytes of one JSON object;
//! at most [`MAX_FRAME`] bytes, at least two (`{}`).
//!
//! The launcher reads requests (written by the stub — or by anything else in the sandbox:
//! the game can connect to the socket itself) and the stub reads replies. Both shapes are
//! exact: an object with exactly the named string fields, nothing else, nothing twice.

use serde::Deserialize;

use super::MAX_FRAME;

/// The smallest body there is: `{}`.
const MIN_FRAME: usize = 2;

/// What the stub sends: the two arguments the client gave it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub start: String,
    pub end: String,
}

/// What the launcher answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// The address for the game.
    Url(String),
    /// Why there is none.
    Error(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyFields {
    url: Option<String>,
    error: Option<String>,
}

/// A frame around `body`, or `None` if the body is too large or too small.
pub fn frame(body: &[u8]) -> Option<Vec<u8>> {
    if !(MIN_FRAME..=MAX_FRAME).contains(&body.len()) {
        return None;
    }
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
    Some(out)
}

/// The length a frame header announces, if it is one the bridge accepts.
pub fn body_len(header: [u8; 4]) -> Result<usize, &'static str> {
    let len = u32::from_be_bytes(header) as usize;
    if len > MAX_FRAME {
        return Err("the frame is larger than 16 KiB");
    }
    if len < MIN_FRAME {
        return Err("the frame is too small to hold a JSON object");
    }
    Ok(len)
}

pub fn request_frame(start: &str, end: &str) -> Option<Vec<u8>> {
    frame(serde_json::json!({ "start": start, "end": end }).to_string().as_bytes())
}

pub fn reply_frame(reply: &Reply) -> Vec<u8> {
    let body = match reply {
        Reply::Url(url) => serde_json::json!({ "url": url }),
        Reply::Error(text) => serde_json::json!({ "error": text }),
    };
    // A reply holds an address of at most 1023 bytes or a short reason; it always fits.
    frame(body.to_string().as_bytes()).unwrap_or_else(|| frame(br#"{"error":"the reply did not fit"}"#).expect("fits"))
}

/// A derived struct reader also takes a JSON array (`["a","b"]`) for the struct's fields in
/// order; the bridge's shapes are objects only.
fn is_object(body: &[u8]) -> bool {
    body.trim_ascii_start().first() == Some(&b'{')
}

pub fn parse_request(body: &[u8]) -> Result<Request, &'static str> {
    const SHAPE: &str = "the request is not {\"start\": …, \"end\": …}";
    if !is_object(body) {
        return Err(SHAPE);
    }
    serde_json::from_slice::<Request>(body).map_err(|_| SHAPE)
}

pub fn parse_reply(body: &[u8]) -> Result<Reply, &'static str> {
    if !is_object(body) {
        return Err("the reply is not {\"url\": …} or {\"error\": …}");
    }
    match serde_json::from_slice::<ReplyFields>(body) {
        Ok(ReplyFields { url: Some(url), error: None }) => Ok(Reply::Url(url)),
        Ok(ReplyFields { url: None, error: Some(error) }) => Ok(Reply::Error(error)),
        _ => Err("the reply is not {\"url\": …} or {\"error\": …}"),
    }
}

/// Read one frame's body from a blocking stream (the stub's side).
pub fn read_frame_blocking(stream: &mut impl std::io::Read) -> std::io::Result<Vec<u8>> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header)?;
    let len = body_len(header).map_err(|why| std::io::Error::new(std::io::ErrorKind::InvalidData, why))?;
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body)?;
    Ok(body)
}

/// Read one frame's body from an asynchronous stream (the launcher's side). The caller bounds
/// the time.
pub async fn read_frame(stream: &mut (impl tokio::io::AsyncRead + Unpin)) -> Result<Vec<u8>, &'static str> {
    use tokio::io::AsyncReadExt;
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).await.map_err(|_| "the connection ended before a whole frame")?;
    let len = body_len(header)?;
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).await.map_err(|_| "the connection ended before a whole frame")?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_replies_round_trip_through_frames() {
        let framed = request_frame("https://login.live.com/x?a=\"b\"", "ms-xal-0000000048183522://auth").unwrap();
        let body = read_frame_blocking(&mut framed.as_slice()).unwrap();
        assert_eq!(
            parse_request(&body).unwrap(),
            Request { start: "https://login.live.com/x?a=\"b\"".into(), end: "ms-xal-0000000048183522://auth".into() }
        );
        for reply in [Reply::Url("ms-xal-0000000048183522://auth?code=a&state=b".into()), Reply::Error("no".into())] {
            let framed = reply_frame(&reply);
            assert_eq!(parse_reply(&read_frame_blocking(&mut framed.as_slice()).unwrap()).unwrap(), reply);
        }
    }

    #[test]
    fn frames_outside_the_bounds_are_refused_before_anything_is_allocated() {
        assert!(body_len((MAX_FRAME as u32 + 1).to_be_bytes()).is_err());
        assert!(body_len(u32::MAX.to_be_bytes()).is_err());
        assert!(body_len(1u32.to_be_bytes()).is_err());
        assert_eq!(body_len(2u32.to_be_bytes()), Ok(2));
        assert!(frame(&vec![b' '; MAX_FRAME + 1]).is_none());
        assert!(request_frame(&"x".repeat(MAX_FRAME), "").is_none());
        // A short read is an error, not a shorter frame.
        let mut framed = request_frame("a", "b").unwrap();
        framed.pop();
        assert!(read_frame_blocking(&mut framed.as_slice()).is_err());
    }

    #[test]
    fn only_the_exact_shapes_are_accepted() {
        for body in [
            &br#"{"start":"a"}"#[..],
            br#"{"start":"a","end":"b","extra":1}"#,
            br#"{"start":"a","end":"b","start":"c"}"#,
            br#"{"start":1,"end":"b"}"#,
            br#"["a","b"]"#,
            br#""a""#,
            br#" ["a","b"]"#,
            b"{\"start\":\"a\",\"end\":\"b\"} trailing",
            b"\xff\xfe",
        ] {
            assert!(parse_request(body).is_err(), "{:?}", String::from_utf8_lossy(body));
        }
        for body in [
            &br#"{}"#[..],
            br#"{"url":"a","error":"b"}"#,
            br#"{"url":null}"#,
            br#"{"url":"a","x":1}"#,
            br#"{"url":"a","url":"b"}"#,
            br#"["a",null]"#,
            br#"[null,"b"]"#,
        ] {
            assert!(parse_reply(body).is_err(), "{:?}", String::from_utf8_lossy(body));
        }
    }
}
