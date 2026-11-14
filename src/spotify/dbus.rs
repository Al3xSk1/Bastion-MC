//! A small client for the D-Bus wire protocol, written for one job: talking to the Spotify
//! desktop app's MPRIS interface through `xdg-dbus-proxy` (see `mpris`).
//!
//! **Why not a crate.** The pure-Rust implementations bring a runtime of their own or a large
//! dependency tree into the binary that holds tokens, and the C library is a C library. What
//! MPRIS needs is method calls, property reads and one kind of signal; this file is that and
//! no more: the marshalling format (both byte orders when reading, little-endian when
//! writing), the SASL `EXTERNAL` handshake (no file-descriptor passing is ever negotiated),
//! and a connection that matches replies to calls and hands signals on.
//!
//! **Everything received is untrusted.** The bus forwards what other programs of the user
//! send — the player, or whatever program owns its name. [`Message::parse`] is strict: every
//! length, alignment, padding byte, boolean, string, object path, signature and nesting depth
//! is checked, nothing is repaired, and the work per message is bounded ([`MAX_MESSAGE`],
//! [`MAX_VALUES`], [`MAX_DEPTH`]). [`fuzz`] runs it on arbitrary bytes.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::{mpsc, oneshot},
};

use crate::error::{Error, Result};

/// The largest message accepted from the bus. MPRIS messages are a few kilobytes.
pub const MAX_MESSAGE: usize = 1 << 20;
/// Values decoded from one message, all levels together.
pub const MAX_VALUES: usize = 1 << 16;
/// Containers (arrays, structs, dict entries, variants) inside each other.
pub const MAX_DEPTH: usize = 32;
/// The specification's limit for a signature.
const MAX_SIGNATURE: usize = 255;
/// Calls waiting for their reply at the same time.
const MAX_PENDING: usize = 32;
/// How long a call waits for its reply.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
/// How long writing one message may take before the connection counts as stuck.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
/// Signals buffered for the consumer; more are dropped (and counted).
const SIGNAL_QUEUE: usize = 64;

fn bad(why: impl Into<String>) -> Error {
    Error::Format(format!("D-Bus: {}", why.into()))
}

// ---------------------------------------------------------------- values and types

/// One D-Bus value. Arrays carry their element signature so that an empty one can still be
/// written; `ay` is kept as bytes.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Byte(u8),
    Bool(bool),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    Double(f64),
    Str(String),
    Path(String),
    Signature(String),
    UnixFd(u32),
    Variant(Box<Value>),
    Bytes(Vec<u8>),
    Array { element: String, items: Vec<Value> },
    Dict { key: String, value: String, entries: Vec<(Value, Value)> },
    Struct(Vec<Value>),
}

impl Value {
    /// The value's type signature.
    pub fn signature(&self) -> String {
        match self {
            Value::Byte(_) => "y".into(),
            Value::Bool(_) => "b".into(),
            Value::I16(_) => "n".into(),
            Value::U16(_) => "q".into(),
            Value::I32(_) => "i".into(),
            Value::U32(_) => "u".into(),
            Value::I64(_) => "x".into(),
            Value::U64(_) => "t".into(),
            Value::Double(_) => "d".into(),
            Value::Str(_) => "s".into(),
            Value::Path(_) => "o".into(),
            Value::Signature(_) => "g".into(),
            Value::UnixFd(_) => "h".into(),
            Value::Variant(_) => "v".into(),
            Value::Bytes(_) => "ay".into(),
            Value::Array { element, .. } => format!("a{element}"),
            Value::Dict { key, value, .. } => format!("a{{{key}{value}}}"),
            Value::Struct(fields) => format!("({})", fields.iter().map(Value::signature).collect::<String>()),
        }
    }

    /// A variant's content (once; a variant inside a variant stays wrapped), anything else as it is.
    pub fn unvariant(&self) -> &Value {
        match self {
            Value::Variant(inner) => inner,
            other => other,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self.unvariant() {
            Value::Str(s) | Value::Path(s) | Value::Signature(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self.unvariant() {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Any integer type, as a signed 64-bit number (a `u64` above `i64::MAX` is not one).
    pub fn as_i64(&self) -> Option<i64> {
        Some(match self.unvariant() {
            Value::Byte(v) => i64::from(*v),
            Value::I16(v) => i64::from(*v),
            Value::U16(v) => i64::from(*v),
            Value::I32(v) => i64::from(*v),
            Value::U32(v) => i64::from(*v),
            Value::I64(v) => *v,
            Value::U64(v) => i64::try_from(*v).ok()?,
            _ => return None,
        })
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self.unvariant() {
            Value::Double(d) if d.is_finite() => Some(*d),
            Value::Double(_) => None,
            other => other.as_i64().map(|v| v as f64),
        }
    }

    /// An array of strings (`as`), or a single string as a list of one.
    pub fn as_str_list(&self) -> Option<Vec<&str>> {
        match self.unvariant() {
            Value::Array { items, .. } => items.iter().map(Value::as_str).collect(),
            Value::Str(s) => Some(vec![s.as_str()]),
            _ => None,
        }
    }

    /// The entries of a dictionary with string keys (`a{sv}`, `a{ss}`, …).
    pub fn as_dict(&self) -> Option<Vec<(&str, &Value)>> {
        match self.unvariant() {
            Value::Dict { entries, .. } => entries.iter().map(|(k, v)| Some((k.as_str()?, v))).collect(),
            _ => None,
        }
    }

    /// `a{sv}` from pairs, for property updates and the like.
    pub fn string_dict(pairs: Vec<(String, Value)>) -> Value {
        Value::Dict {
            key: "s".into(),
            value: "v".into(),
            entries: pairs.into_iter().map(|(k, v)| (Value::Str(k), Value::Variant(Box::new(v)))).collect(),
        }
    }
}

/// A complete type, parsed from a signature.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Type {
    Byte,
    Bool,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    Double,
    Str,
    Path,
    Signature,
    UnixFd,
    Variant,
    Array(Box<Type>),
    Dict(Box<Type>, Box<Type>),
    Struct(Vec<Type>),
}

impl Type {
    fn alignment(&self) -> usize {
        match self {
            Type::Byte | Type::Signature | Type::Variant => 1,
            Type::I16 | Type::U16 => 2,
            Type::Bool | Type::I32 | Type::U32 | Type::Str | Type::Path | Type::UnixFd | Type::Array(_) => 4,
            Type::Dict(..) => 4,
            Type::I64 | Type::U64 | Type::Double | Type::Struct(_) => 8,
        }
    }

    fn is_basic(&self) -> bool {
        !matches!(self, Type::Variant | Type::Array(_) | Type::Dict(..) | Type::Struct(_))
    }

    fn write(&self, out: &mut String) {
        match self {
            Type::Byte => out.push('y'),
            Type::Bool => out.push('b'),
            Type::I16 => out.push('n'),
            Type::U16 => out.push('q'),
            Type::I32 => out.push('i'),
            Type::U32 => out.push('u'),
            Type::I64 => out.push('x'),
            Type::U64 => out.push('t'),
            Type::Double => out.push('d'),
            Type::Str => out.push('s'),
            Type::Path => out.push('o'),
            Type::Signature => out.push('g'),
            Type::UnixFd => out.push('h'),
            Type::Variant => out.push('v'),
            Type::Array(element) => {
                out.push('a');
                element.write(out);
            }
            Type::Dict(key, value) => {
                out.push_str("a{");
                key.write(out);
                value.write(out);
                out.push('}');
            }
            Type::Struct(fields) => {
                out.push('(');
                for field in fields {
                    field.write(out);
                }
                out.push(')');
            }
        }
    }

    fn signature(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }
}

/// Parse a signature into its complete types. Strict: the specification's grammar, at most
/// [`MAX_SIGNATURE`] bytes, arrays and structs each nested at most 32 deep.
fn parse_signature(sig: &str) -> Result<Vec<Type>> {
    if sig.len() > MAX_SIGNATURE {
        return Err(bad("signature too long"));
    }
    let bytes = sig.as_bytes();
    let mut at = 0;
    let mut types = Vec::new();
    while at < bytes.len() {
        types.push(parse_one(bytes, &mut at, 0, 0)?);
    }
    Ok(types)
}

fn parse_one(sig: &[u8], at: &mut usize, arrays: usize, structs: usize) -> Result<Type> {
    let Some(&c) = sig.get(*at) else { return Err(bad("incomplete signature")) };
    *at += 1;
    Ok(match c {
        b'y' => Type::Byte,
        b'b' => Type::Bool,
        b'n' => Type::I16,
        b'q' => Type::U16,
        b'i' => Type::I32,
        b'u' => Type::U32,
        b'x' => Type::I64,
        b't' => Type::U64,
        b'd' => Type::Double,
        b's' => Type::Str,
        b'o' => Type::Path,
        b'g' => Type::Signature,
        b'h' => Type::UnixFd,
        b'v' => Type::Variant,
        b'a' => {
            if arrays >= 32 {
                return Err(bad("arrays nested too deep in a signature"));
            }
            if sig.get(*at) == Some(&b'{') {
                *at += 1;
                let key = parse_one(sig, at, arrays + 1, structs)?;
                if !key.is_basic() {
                    return Err(bad("a dictionary key must be a basic type"));
                }
                let value = parse_one(sig, at, arrays + 1, structs)?;
                if sig.get(*at) != Some(&b'}') {
                    return Err(bad("a dictionary entry has exactly two types"));
                }
                *at += 1;
                Type::Dict(Box::new(key), Box::new(value))
            } else {
                Type::Array(Box::new(parse_one(sig, at, arrays + 1, structs)?))
            }
        }
        b'(' => {
            if structs >= 32 {
                return Err(bad("structs nested too deep in a signature"));
            }
            let mut fields = Vec::new();
            loop {
                match sig.get(*at) {
                    Some(b')') => {
                        *at += 1;
                        break;
                    }
                    Some(_) => fields.push(parse_one(sig, at, arrays, structs + 1)?),
                    None => return Err(bad("unterminated struct in a signature")),
                }
            }
            if fields.is_empty() {
                return Err(bad("empty struct in a signature"));
            }
            Type::Struct(fields)
        }
        _ => return Err(bad("unknown type code in a signature")),
    })
}

// ---------------------------------------------------------------- names

fn is_element(part: &str, digits_first: bool, dash: bool) -> bool {
    !part.is_empty()
        && (digits_first || !part.as_bytes()[0].is_ascii_digit())
        && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || (dash && b == b'-'))
}

/// An interface or error name: two or more elements separated by dots.
pub fn is_interface_name(name: &str) -> bool {
    name.len() <= 255 && name.split('.').count() >= 2 && name.split('.').all(|p| is_element(p, false, false))
}

pub fn is_member_name(name: &str) -> bool {
    name.len() <= 255 && is_element(name, false, false)
}

/// A unique (`:1.42`) or well-known (`org.example.Name`) bus name.
pub fn is_bus_name(name: &str) -> bool {
    if name.len() > 255 {
        return false;
    }
    match name.strip_prefix(':') {
        Some(rest) => rest.split('.').count() >= 2 && rest.split('.').all(|p| is_element(p, true, true)),
        None => name.split('.').count() >= 2 && name.split('.').all(|p| is_element(p, false, true)),
    }
}

pub fn is_object_path(path: &str) -> bool {
    path == "/"
        || (path.starts_with('/')
            && !path.ends_with('/')
            && path[1..].split('/').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')))
}

// ---------------------------------------------------------------- reading

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    big: bool,
    values: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|end| *end <= self.buf.len()).ok_or_else(|| bad("truncated"))?;
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    /// Skip to a multiple of `n` (relative to the message start); padding must be zero.
    fn align(&mut self, n: usize) -> Result<()> {
        let pad = (n - self.pos % n) % n;
        if self.take(pad)?.iter().any(|b| *b != 0) {
            return Err(bad("non-zero padding"));
        }
        Ok(())
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn fixed<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.align(N)?;
        let mut raw: [u8; N] = self.take(N)?.try_into().expect("N bytes");
        if self.big != cfg!(target_endian = "big") {
            raw.reverse();
        }
        Ok(raw)
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_ne_bytes(self.fixed::<4>()?))
    }

    fn text(&mut self, len: usize) -> Result<String> {
        let raw = self.take(len)?;
        if self.u8()? != 0 {
            return Err(bad("string without its terminating NUL"));
        }
        if raw.contains(&0) {
            return Err(bad("NUL inside a string"));
        }
        String::from_utf8(raw.to_vec()).map_err(|_| bad("string is not UTF-8"))
    }

    fn string(&mut self) -> Result<String> {
        let len = self.u32()? as usize;
        if len > MAX_MESSAGE {
            return Err(bad("string longer than a message"));
        }
        self.text(len)
    }

    fn signature(&mut self) -> Result<String> {
        let len = usize::from(self.u8()?);
        let sig = self.text(len)?;
        parse_signature(&sig)?;
        Ok(sig)
    }

    fn value(&mut self, t: &Type, depth: usize) -> Result<Value> {
        self.values += 1;
        if self.values > MAX_VALUES {
            return Err(bad("too many values in one message"));
        }
        if depth > MAX_DEPTH {
            return Err(bad("values nested too deep"));
        }
        Ok(match t {
            Type::Byte => Value::Byte(self.u8()?),
            Type::Bool => match self.u32()? {
                0 => Value::Bool(false),
                1 => Value::Bool(true),
                _ => return Err(bad("a boolean that is neither 0 nor 1")),
            },
            Type::I16 => Value::I16(i16::from_ne_bytes(self.fixed::<2>()?)),
            Type::U16 => Value::U16(u16::from_ne_bytes(self.fixed::<2>()?)),
            Type::I32 => Value::I32(i32::from_ne_bytes(self.fixed::<4>()?)),
            Type::U32 => Value::U32(self.u32()?),
            Type::I64 => Value::I64(i64::from_ne_bytes(self.fixed::<8>()?)),
            Type::U64 => Value::U64(u64::from_ne_bytes(self.fixed::<8>()?)),
            Type::Double => Value::Double(f64::from_ne_bytes(self.fixed::<8>()?)),
            Type::UnixFd => Value::UnixFd(self.u32()?),
            Type::Str => Value::Str(self.string()?),
            Type::Path => {
                let path = self.string()?;
                if !is_object_path(&path) {
                    return Err(bad("malformed object path"));
                }
                Value::Path(path)
            }
            Type::Signature => Value::Signature(self.signature()?),
            Type::Variant => {
                let sig = self.signature()?;
                let mut types = parse_signature(&sig)?;
                if types.len() != 1 {
                    return Err(bad("a variant holds exactly one complete type"));
                }
                let inner = types.pop().expect("one type");
                Value::Variant(Box::new(self.value(&inner, depth + 1)?))
            }
            Type::Struct(fields) => {
                self.align(8)?;
                let mut values = Vec::with_capacity(fields.len());
                for field in fields {
                    values.push(self.value(field, depth + 1)?);
                }
                Value::Struct(values)
            }
            Type::Array(element) => {
                let (len, end) = self.array_bounds(element.alignment())?;
                if **element == Type::Byte {
                    return Ok(Value::Bytes(self.take(len)?.to_vec()));
                }
                let mut items = Vec::new();
                while self.pos < end {
                    items.push(self.value(element, depth + 1)?);
                }
                if self.pos != end {
                    return Err(bad("an array element runs past the array"));
                }
                Value::Array { element: element.signature(), items }
            }
            Type::Dict(key, value) => {
                let (_, end) = self.array_bounds(8)?;
                let mut entries = Vec::new();
                while self.pos < end {
                    self.align(8)?;
                    self.values += 1;
                    let k = self.value(key, depth + 2)?;
                    let v = self.value(value, depth + 2)?;
                    entries.push((k, v));
                }
                if self.pos != end {
                    return Err(bad("a dictionary entry runs past the array"));
                }
                Value::Dict { key: key.signature(), value: value.signature(), entries }
            }
        })
    }

    /// An array's byte length, then the padding to its first element (which the length does
    /// not count). Returns the length and where the array ends.
    fn array_bounds(&mut self, element_alignment: usize) -> Result<(usize, usize)> {
        let len = self.u32()? as usize;
        if len > (1 << 26) {
            return Err(bad("array longer than the specification allows"));
        }
        self.align(element_alignment)?;
        let end =
            self.pos.checked_add(len).filter(|end| *end <= self.buf.len()).ok_or_else(|| bad("truncated array"))?;
        Ok((len, end))
    }
}

// ---------------------------------------------------------------- writing (little-endian)

struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn align(&mut self, n: usize) {
        while !self.buf.len().is_multiple_of(n) {
            self.buf.push(0);
        }
    }

    fn u32(&mut self, v: u32) {
        self.align(4);
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn string(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
    }

    fn signature(&mut self, s: &str) {
        self.buf.push(s.len() as u8);
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
    }

    fn value(&mut self, v: &Value) -> Result<()> {
        match v {
            Value::Byte(b) => self.buf.push(*b),
            Value::Bool(b) => self.u32(u32::from(*b)),
            Value::I16(n) => {
                self.align(2);
                self.buf.extend_from_slice(&n.to_le_bytes());
            }
            Value::U16(n) => {
                self.align(2);
                self.buf.extend_from_slice(&n.to_le_bytes());
            }
            Value::I32(n) => {
                self.align(4);
                self.buf.extend_from_slice(&n.to_le_bytes());
            }
            Value::U32(n) | Value::UnixFd(n) => self.u32(*n),
            Value::I64(n) => {
                self.align(8);
                self.buf.extend_from_slice(&n.to_le_bytes());
            }
            Value::U64(n) => {
                self.align(8);
                self.buf.extend_from_slice(&n.to_le_bytes());
            }
            Value::Double(d) => {
                self.align(8);
                self.buf.extend_from_slice(&d.to_le_bytes());
            }
            Value::Str(s) => {
                if s.contains('\0') {
                    return Err(bad("NUL inside a string to send"));
                }
                self.string(s)
            }
            Value::Path(p) => {
                if !is_object_path(p) {
                    return Err(bad("malformed object path to send"));
                }
                self.string(p)
            }
            Value::Signature(s) => {
                parse_signature(s)?;
                self.signature(s)
            }
            Value::Variant(inner) => {
                let sig = inner.signature();
                parse_signature(&sig)?;
                self.signature(&sig);
                self.value(inner)?;
            }
            Value::Bytes(bytes) => {
                self.u32(bytes.len() as u32);
                self.buf.extend_from_slice(bytes);
            }
            Value::Array { element, items } => {
                let types = parse_signature(element)?;
                let [t] = types.as_slice() else { return Err(bad("an array has one element type")) };
                self.u32(0);
                let at = self.buf.len() - 4;
                self.align(t.alignment());
                let start = self.buf.len();
                for item in items {
                    if item.signature() != *element {
                        return Err(bad("an array element of the wrong type"));
                    }
                    self.value(item)?;
                }
                let len = (self.buf.len() - start) as u32;
                self.buf[at..at + 4].copy_from_slice(&len.to_le_bytes());
            }
            Value::Dict { key, value, entries } => {
                self.u32(0);
                let at = self.buf.len() - 4;
                self.align(8);
                let start = self.buf.len();
                for (k, v) in entries {
                    if k.signature() != *key || v.signature() != *value {
                        return Err(bad("a dictionary entry of the wrong type"));
                    }
                    self.align(8);
                    self.value(k)?;
                    self.value(v)?;
                }
                let len = (self.buf.len() - start) as u32;
                self.buf[at..at + 4].copy_from_slice(&len.to_le_bytes());
            }
            Value::Struct(fields) => {
                self.align(8);
                for field in fields {
                    self.value(field)?;
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- messages

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    MethodCall,
    Return,
    Error,
    Signal,
    /// A type this client does not know; the specification says to ignore such messages.
    Unknown(u8),
}

impl Kind {
    fn code(self) -> u8 {
        match self {
            Kind::MethodCall => 1,
            Kind::Return => 2,
            Kind::Error => 3,
            Kind::Signal => 4,
            Kind::Unknown(code) => code,
        }
    }
}

/// `NO_REPLY_EXPECTED`.
pub const FLAG_NO_REPLY: u8 = 0x1;
/// `NO_AUTO_START`: never have the bus start a program for a call.
pub const FLAG_NO_AUTO_START: u8 = 0x2;

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub kind: Kind,
    pub flags: u8,
    pub serial: u32,
    pub path: Option<String>,
    pub interface: Option<String>,
    pub member: Option<String>,
    pub error_name: Option<String>,
    pub reply_serial: Option<u32>,
    pub destination: Option<String>,
    pub sender: Option<String>,
    pub body: Vec<Value>,
}

/// The total length of the message whose first 16 bytes are `head`, from its fixed header:
/// what a reader must read before it can parse. Refuses anything over [`MAX_MESSAGE`].
pub fn message_length(head: &[u8; 16]) -> Result<usize> {
    let big = match head[0] {
        b'l' => false,
        b'B' => true,
        _ => return Err(bad("unknown byte order")),
    };
    let word = |at: usize| {
        let raw: [u8; 4] = head[at..at + 4].try_into().expect("four bytes");
        if big { u32::from_be_bytes(raw) } else { u32::from_le_bytes(raw) }
    };
    if head[3] != 1 {
        return Err(bad("unknown protocol version"));
    }
    let (body, fields) = (word(4) as usize, word(12) as usize);
    let total = 16usize
        .checked_add(fields)
        .map(|n| n.div_ceil(8) * 8)
        .and_then(|n| n.checked_add(body))
        .ok_or_else(|| bad("impossible message length"))?;
    if total > MAX_MESSAGE {
        return Err(bad(format!("a message of {total} bytes (limit {MAX_MESSAGE})")));
    }
    Ok(total)
}

impl Message {
    /// A method call (no flags set but `NO_AUTO_START`).
    pub fn call(destination: &str, path: &str, interface: &str, member: &str, body: Vec<Value>) -> Message {
        Message {
            kind: Kind::MethodCall,
            flags: FLAG_NO_AUTO_START,
            serial: 0,
            path: Some(path.into()),
            interface: Some(interface.into()),
            member: Some(member.into()),
            error_name: None,
            reply_serial: None,
            destination: Some(destination.into()),
            sender: None,
            body,
        }
    }

    fn signature(&self) -> String {
        self.body.iter().map(Value::signature).collect()
    }

    /// The message on the wire, little-endian. Checks what it writes as strictly as
    /// [`Message::parse`] checks what it reads.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.serial == 0 {
            return Err(bad("a message to send needs a serial"));
        }
        let mut body = Writer { buf: Vec::new() };
        for value in &self.body {
            body.value(value)?;
        }
        let signature = self.signature();
        parse_signature(&signature)?;
        let mut fields: Vec<(u8, Value)> = Vec::new();
        if let Some(path) = &self.path {
            fields.push((1, Value::Path(path.clone())));
        }
        for (code, text, valid) in [
            (2u8, &self.interface, is_interface_name as fn(&str) -> bool),
            (3, &self.member, is_member_name),
            (4, &self.error_name, is_interface_name),
            (6, &self.destination, is_bus_name),
            (7, &self.sender, is_bus_name),
        ] {
            if let Some(text) = text {
                if !valid(text) {
                    return Err(bad(format!("malformed header field {code} to send")));
                }
                fields.push((code, Value::Str(text.clone())));
            }
        }
        if let Some(serial) = self.reply_serial {
            fields.push((5, Value::U32(serial)));
        }
        if !signature.is_empty() {
            fields.push((8, Value::Signature(signature)));
        }
        let mut w = Writer { buf: vec![b'l', self.kind.code(), self.flags, 1] };
        w.buf.extend_from_slice(&(body.buf.len() as u32).to_le_bytes());
        w.buf.extend_from_slice(&self.serial.to_le_bytes());
        w.value(&Value::Array {
            element: "(yv)".into(),
            items: fields
                .into_iter()
                .map(|(code, value)| Value::Struct(vec![Value::Byte(code), Value::Variant(Box::new(value))]))
                .collect(),
        })?;
        w.align(8);
        w.buf.extend_from_slice(&body.buf);
        if w.buf.len() > MAX_MESSAGE {
            return Err(bad("message to send is too large"));
        }
        Ok(w.buf)
    }

    /// One complete message (exactly [`message_length`] bytes).
    pub fn parse(buf: &[u8]) -> Result<Message> {
        if buf.len() < 16 {
            return Err(bad("truncated header"));
        }
        let total = message_length(buf[..16].try_into().expect("16 bytes"))?;
        if total != buf.len() {
            return Err(bad("the message is not as long as its header says"));
        }
        let mut r = Reader { buf, pos: 0, big: buf[0] == b'B', values: 0 };
        r.take(1)?;
        let kind = match r.u8()? {
            0 => return Err(bad("message type 0")),
            1 => Kind::MethodCall,
            2 => Kind::Return,
            3 => Kind::Error,
            4 => Kind::Signal,
            other => Kind::Unknown(other),
        };
        let flags = r.u8()?;
        r.take(1)?; // version, checked by message_length
        let body_len = r.u32()? as usize;
        let serial = r.u32()?;
        if serial == 0 {
            return Err(bad("serial 0"));
        }
        let fields = r.value(&Type::Array(Box::new(Type::Struct(vec![Type::Byte, Type::Variant]))), 0)?;
        let Value::Array { items, .. } = fields else { unreachable!("an array type reads as an array") };
        let mut m = Message {
            kind,
            flags,
            serial,
            path: None,
            interface: None,
            member: None,
            error_name: None,
            reply_serial: None,
            destination: None,
            sender: None,
            body: Vec::new(),
        };
        let mut signature = None;
        let mut seen = [false; 10];
        for item in items {
            let Value::Struct(pair) = item else { unreachable!("(yv) reads as a struct") };
            let (Value::Byte(code), Value::Variant(value)) = (&pair[0], &pair[1]) else { unreachable!("(yv)") };
            let code = *code;
            if (1..=9).contains(&code) {
                if seen[usize::from(code)] {
                    return Err(bad(format!("header field {code} twice")));
                }
                seen[usize::from(code)] = true;
            }
            match (code, value.as_ref()) {
                (1, Value::Path(p)) => m.path = Some(p.clone()),
                (2, Value::Str(s)) if is_interface_name(s) => m.interface = Some(s.clone()),
                (3, Value::Str(s)) if is_member_name(s) => m.member = Some(s.clone()),
                (4, Value::Str(s)) if is_interface_name(s) => m.error_name = Some(s.clone()),
                (5, Value::U32(n)) if *n != 0 => m.reply_serial = Some(*n),
                (6, Value::Str(s)) if is_bus_name(s) => m.destination = Some(s.clone()),
                (7, Value::Str(s)) if is_bus_name(s) => m.sender = Some(s.clone()),
                (8, Value::Signature(s)) => signature = Some(s.clone()),
                // This client never negotiates descriptor passing: a message that says it
                // carries descriptors cannot be one meant for it.
                (9, Value::U32(0)) => {}
                (9, Value::U32(_)) => return Err(bad("a message with file descriptors")),
                (1..=9, _) => return Err(bad(format!("header field {code} of the wrong type or shape"))),
                // Unknown fields are ignored, as the specification requires.
                _ => {}
            }
        }
        let required = match kind {
            Kind::MethodCall => m.path.is_some() && m.member.is_some(),
            Kind::Return => m.reply_serial.is_some(),
            Kind::Error => m.error_name.is_some() && m.reply_serial.is_some(),
            Kind::Signal => m.path.is_some() && m.interface.is_some() && m.member.is_some(),
            Kind::Unknown(_) => true,
        };
        if !required {
            return Err(bad("a required header field is missing"));
        }
        r.align(8)?;
        if r.buf.len() - r.pos != body_len {
            return Err(bad("body length mismatch"));
        }
        let types = parse_signature(signature.as_deref().unwrap_or(""))?;
        for t in &types {
            m.body.push(r.value(t, 0)?);
        }
        if r.pos != r.buf.len() {
            return Err(bad("bytes left over after the body"));
        }
        Ok(m)
    }

    /// For an error reply: its name and, if the first argument is a string, that text
    /// (sanitised and short).
    pub fn error_text(&self) -> String {
        let detail = self.body.first().and_then(Value::as_str).unwrap_or("");
        format!(
            "{}{}{}",
            self.error_name.as_deref().unwrap_or("error"),
            if detail.is_empty() { "" } else { ": " },
            crate::util::sanitize_line(detail.as_bytes(), 160)
        )
    }
}

// ---------------------------------------------------------------- connection

/// `AUTH EXTERNAL` with the uid as the initial response, then `BEGIN`. No unix-fd
/// negotiation: nothing this client sends or accepts carries descriptors.
async fn authenticate(stream: &mut UnixStream, uid: u32) -> Result<()> {
    let hex: String = uid.to_string().bytes().map(|b| format!("{b:02x}")).collect();
    let io = |e: std::io::Error| Error::Net(format!("D-Bus handshake: {e}"));
    stream.write_all(format!("\0AUTH EXTERNAL {hex}\r\n").as_bytes()).await.map_err(io)?;
    // One line, read a byte at a time: nothing else is sent before BEGIN.
    let mut line = Vec::with_capacity(64);
    while !line.ends_with(b"\r\n") {
        if line.len() > 512 {
            return Err(bad("handshake line too long"));
        }
        line.push(stream.read_u8().await.map_err(io)?);
    }
    let line = String::from_utf8_lossy(&line[..line.len() - 2]).into_owned();
    match line.strip_prefix("OK ") {
        Some(guid) if guid.len() == 32 && guid.bytes().all(|b| b.is_ascii_hexdigit()) => {}
        _ => {
            return Err(Error::Net(format!(
                "D-Bus handshake refused: {}",
                crate::util::sanitize_line(line.as_bytes(), 120)
            )));
        }
    }
    stream.write_all(b"BEGIN\r\n").await.map_err(io)?;
    Ok(())
}

#[derive(Default)]
struct Pending {
    calls: HashMap<u32, oneshot::Sender<Message>>,
    closed: Option<String>,
}

/// Counters of what the connection did not deliver.
#[derive(Debug, Default)]
pub struct Stats {
    pub signals_dropped: AtomicU64,
    pub calls_ignored: AtomicU64,
}

/// A connection to a bus (here: to xdg-dbus-proxy). Calls can be made from any task; the
/// signals arrive on the receiver returned by [`Connection::open`].
pub struct Connection {
    writer: tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>,
    pending: Arc<Mutex<Pending>>,
    serial: AtomicU32,
    reader: tokio::task::JoinHandle<()>,
    pub stats: Arc<Stats>,
    /// The name the bus gave this connection.
    pub unique_name: String,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

fn locked(pending: &Mutex<Pending>) -> std::sync::MutexGuard<'_, Pending> {
    pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

async fn read_loop(
    mut reader: tokio::net::unix::OwnedReadHalf,
    pending: Arc<Mutex<Pending>>,
    signals: mpsc::Sender<Message>,
    stats: Arc<Stats>,
) {
    let outcome: Result<()> = async {
        loop {
            let mut head = [0u8; 16];
            reader.read_exact(&mut head).await.map_err(|e| Error::Net(format!("D-Bus: {e}")))?;
            let total = message_length(&head)?;
            let mut buf = vec![0u8; total];
            buf[..16].copy_from_slice(&head);
            reader.read_exact(&mut buf[16..]).await.map_err(|e| Error::Net(format!("D-Bus: {e}")))?;
            let message = Message::parse(&buf)?;
            match message.kind {
                Kind::Return | Kind::Error => {
                    let waiter = message.reply_serial.and_then(|serial| locked(&pending).calls.remove(&serial));
                    if let Some(waiter) = waiter {
                        let _ = waiter.send(message);
                    }
                }
                Kind::Signal => {
                    if signals.try_send(message).is_err() {
                        stats.signals_dropped.fetch_add(1, Ordering::Relaxed);
                    }
                }
                // Nothing is offered on this connection; a caller waits for its timeout.
                Kind::MethodCall => {
                    stats.calls_ignored.fetch_add(1, Ordering::Relaxed);
                }
                Kind::Unknown(_) => {}
            }
        }
    }
    .await;
    let why = match outcome {
        Ok(()) => "closed".to_string(),
        Err(e) => e.to_string(),
    };
    let mut pending = locked(&pending);
    pending.closed = Some(why);
    // Every waiting call learns that no reply will come.
    pending.calls.clear();
}

impl Connection {
    /// Authenticate as `uid`, start reading, and say `Hello` to the bus.
    pub async fn open(mut stream: UnixStream, uid: u32) -> Result<(Connection, mpsc::Receiver<Message>)> {
        tokio::time::timeout(CALL_TIMEOUT, authenticate(&mut stream, uid))
            .await
            .map_err(|_| Error::Net("D-Bus handshake timed out".into()))??;
        let (read, write) = stream.into_split();
        let pending = Arc::new(Mutex::new(Pending::default()));
        let stats = Arc::new(Stats::default());
        let (tx, rx) = mpsc::channel(SIGNAL_QUEUE);
        let reader = tokio::spawn(read_loop(read, pending.clone(), tx, stats.clone()));
        let mut connection = Connection {
            writer: tokio::sync::Mutex::new(write),
            pending,
            serial: AtomicU32::new(0),
            reader,
            stats,
            unique_name: String::new(),
        };
        let hello = connection
            .call(Message::call(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "Hello",
                vec![],
            ))
            .await?;
        match hello.body.first().and_then(Value::as_str) {
            Some(name) if is_bus_name(name) && name.starts_with(':') => connection.unique_name = name.to_string(),
            _ => return Err(bad("the bus did not answer Hello with a unique name")),
        }
        Ok((connection, rx))
    }

    /// Whether the connection has ended (and why).
    pub fn closed(&self) -> Option<String> {
        locked(&self.pending).closed.clone()
    }

    async fn write(&self, bytes: &[u8]) -> Result<()> {
        let mut writer = self.writer.lock().await;
        tokio::time::timeout(WRITE_TIMEOUT, writer.write_all(bytes))
            .await
            .map_err(|_| Error::Net("D-Bus: the bus does not take what is written to it".into()))?
            .map_err(|e| Error::Net(format!("D-Bus: {e}")))
    }

    /// Call a method and wait for its reply. An error reply is an `Err` with the error's name.
    pub async fn call(&self, mut message: Message) -> Result<Message> {
        // Serials need only be unique among the calls in flight, not ordered on the wire.
        let serial = self.serial.fetch_add(1, Ordering::Relaxed).wrapping_add(1).max(1);
        message.serial = serial;
        let bytes = message.encode()?;
        let (tx, rx) = oneshot::channel();
        // Registered before the call is written, so that a reply cannot overtake it.
        {
            let mut pending = locked(&self.pending);
            if let Some(why) = &pending.closed {
                return Err(Error::Net(format!("D-Bus connection {why}")));
            }
            if pending.calls.len() >= MAX_PENDING {
                return Err(Error::Net("D-Bus: too many calls waiting for replies".into()));
            }
            pending.calls.insert(serial, tx);
        }
        if let Err(e) = self.write(&bytes).await {
            locked(&self.pending).calls.remove(&serial);
            return Err(e);
        }
        let reply = match tokio::time::timeout(CALL_TIMEOUT, rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => {
                return Err(Error::Net(format!(
                    "D-Bus connection {}",
                    self.closed().unwrap_or_else(|| "closed".into())
                )));
            }
            Err(_) => {
                locked(&self.pending).calls.remove(&serial);
                return Err(Error::Net("D-Bus: no reply in time".into()));
            }
        };
        if reply.kind == Kind::Error {
            return Err(Error::Other(format!("D-Bus {}", reply.error_text())));
        }
        Ok(reply)
    }
}

// ---------------------------------------------------------------- fuzzing

/// The parser on arbitrary bytes: it returns, and what it accepts it can write again and
/// read back to the same message (byte order aside).
#[doc(hidden)]
pub fn fuzz(data: &[u8]) {
    if data.len() >= 16 {
        let _ = message_length(data[..16].try_into().expect("16 bytes"));
    }
    let Ok(message) = Message::parse(data) else { return };
    if message.serial == 0 || matches!(message.kind, Kind::Unknown(_)) {
        return;
    }
    // Writing refuses a few things reading accepts (unknown header fields are dropped, a
    // UNIX_FDS field of 0 is not written); what it writes must parse to the same message —
    // compared as bytes, written again (values can hold a NaN, which equals nothing).
    if let Ok(again) = message.encode() {
        let reparsed = Message::parse(&again).expect("a message this client wrote must parse");
        assert_eq!(reparsed.encode().expect("and write again"), again, "a round trip changed the message");
        assert_eq!((reparsed.kind, reparsed.serial), (message.kind, message.serial));
    }
    for value in &message.body {
        let _ = (value.as_dict(), value.as_str_list(), value.as_f64(), value.as_i64());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(message: &Message) -> Message {
        let bytes = message.encode().unwrap();
        assert_eq!(message_length(bytes[..16].try_into().unwrap()).unwrap(), bytes.len());
        Message::parse(&bytes).unwrap()
    }

    #[test]
    fn a_method_call_is_written_exactly_as_the_specification_lays_it_out() {
        let mut m =
            Message::call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "Hello", vec![]);
        m.serial = 1;
        let bytes = m.encode().unwrap();
        // Fixed header: 'l', METHOD_CALL, NO_AUTO_START, version 1, body length 0, serial 1.
        assert_eq!(&bytes[..12], &[b'l', 1, 2, 1, 0, 0, 0, 0, 1, 0, 0, 0]);
        // Then the header field array; its first field is PATH (code 1, signature "o").
        let fields_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        assert_eq!(&bytes[16..20], &[1, 1, b'o', 0]);
        assert_eq!(bytes.len(), (16 + fields_len).div_ceil(8) * 8, "padded to 8, no body");
        assert_eq!(roundtrip(&m), m);
    }

    #[test]
    fn every_type_survives_a_round_trip() {
        let metadata = Value::string_dict(vec![
            ("mpris:trackid".into(), Value::Path("/com/spotify/track/4uLU6hMCjMI75M1A2tKUQC".into())),
            ("mpris:length".into(), Value::U64(213_000_000)),
            ("xesam:artist".into(), Value::Array { element: "s".into(), items: vec![Value::Str("Daft Punk".into())] }),
            ("xesam:title".into(), Value::Str("Harder, Better – ☕".into())),
            ("xesam:autoRating".into(), Value::Double(0.42)),
        ]);
        let mut m = Message {
            kind: Kind::Signal,
            flags: 0,
            serial: 7,
            path: Some("/org/mpris/MediaPlayer2".into()),
            interface: Some("org.freedesktop.DBus.Properties".into()),
            member: Some("PropertiesChanged".into()),
            error_name: None,
            reply_serial: None,
            destination: None,
            sender: Some(":1.42".into()),
            body: vec![
                Value::Str("org.mpris.MediaPlayer2.Player".into()),
                Value::string_dict(vec![
                    ("Metadata".into(), metadata),
                    ("Volume".into(), Value::Double(0.5)),
                    ("Shuffle".into(), Value::Bool(true)),
                    ("Rate".into(), Value::I16(-3)),
                    ("Q".into(), Value::U16(9)),
                    ("Y".into(), Value::Byte(255)),
                    ("Pos".into(), Value::I64(-12)),
                    ("Code".into(), Value::I32(-7)),
                    ("Sig".into(), Value::Signature("a{sv}".into())),
                    ("Raw".into(), Value::Bytes(vec![1, 2, 3])),
                    ("Pair".into(), Value::Struct(vec![Value::U32(1), Value::Str("x".into())])),
                    ("Nested".into(), Value::Variant(Box::new(Value::U32(5)))),
                ]),
                Value::Array { element: "s".into(), items: vec![] },
            ],
        };
        assert_eq!(roundtrip(&m), m);
        m.kind = Kind::Error;
        m.error_name = Some("org.mpris.MediaPlayer2.Error.Nope".into());
        m.reply_serial = Some(3);
        assert_eq!(roundtrip(&m), m);
    }

    /// The same message in big-endian, written by hand: a reader must accept both orders.
    #[test]
    fn big_endian_messages_are_read() {
        let mut m = Message::call(":1.1", "/a", "a.b", "C", vec![Value::U32(0x0102_0304), Value::Str("hi".into())]);
        m.serial = 0x0a0b_0c0d;
        let little = m.encode().unwrap();
        // Re-encode by hand in big-endian: every multi-byte integer flipped. Easiest via a
        // tiny independent writer for this one shape.
        let mut big = vec![b'B', 1, FLAG_NO_AUTO_START, 1];
        let mut body = 0x0102_0304u32.to_be_bytes().to_vec();
        body.extend_from_slice(&2u32.to_be_bytes());
        body.extend_from_slice(b"hi\0");
        big.extend_from_slice(&(body.len() as u32).to_be_bytes());
        big.extend_from_slice(&0x0a0b_0c0du32.to_be_bytes());
        let mut fields: Vec<u8> = Vec::new();
        let pad8 = |v: &mut Vec<u8>| {
            while !(16 + v.len()).is_multiple_of(8) {
                v.push(0)
            }
        };
        let field_str = |v: &mut Vec<u8>, code: u8, sig: u8, text: &str| {
            pad8(v);
            v.extend_from_slice(&[code, 1, sig, 0]);
            v.extend_from_slice(&(text.len() as u32).to_be_bytes());
            v.extend_from_slice(text.as_bytes());
            v.push(0);
        };
        field_str(&mut fields, 1, b'o', "/a");
        field_str(&mut fields, 2, b's', "a.b");
        field_str(&mut fields, 3, b's', "C");
        field_str(&mut fields, 6, b's', ":1.1");
        pad8(&mut fields);
        fields.extend_from_slice(&[8, 1, b'g', 0, 2, b'u', b's', 0]);
        big.extend_from_slice(&(fields.len() as u32).to_be_bytes());
        big.extend_from_slice(&fields);
        while !big.len().is_multiple_of(8) {
            big.push(0);
        }
        big.extend_from_slice(&body);
        assert_eq!(Message::parse(&big).unwrap(), Message::parse(&little).unwrap());
    }

    #[test]
    fn malformed_messages_are_refused_not_repaired() {
        let mut good = Message::call(":1.1", "/a", "a.b", "C", vec![Value::Bool(true), Value::Str("x".into())]);
        good.serial = 1;
        let bytes = good.encode().unwrap();
        assert!(Message::parse(&bytes).is_ok());
        let body_at = bytes.len() - 4 - 6; // "b" (4) then "s": len 4 + 'x' + NUL
        let refused = |mutate: &dyn Fn(&mut Vec<u8>), why: &str| {
            let mut b = bytes.clone();
            mutate(&mut b);
            assert!(Message::parse(&b).is_err(), "{why}");
        };
        refused(&|b| b[0] = b'x', "byte order");
        refused(&|b| b[3] = 2, "protocol version");
        refused(&|b| b[8..12].copy_from_slice(&[0; 4]), "serial 0");
        refused(&|b| b[body_at] = 2, "a boolean of 2");
        refused(&|b| b.truncate(b.len() - 1), "truncated");
        refused(&|b| b.push(0), "trailing byte");
        refused(&|b| *b.last_mut().unwrap() = b'y', "string without NUL");
        // The first header field is PATH "/a": code, signature "o", length 2, "/a\0" ending
        // at offset 27; the next field starts at 32, so 27..32 is padding.
        refused(&|b| b[27] = 1, "non-zero padding in the header fields");
        // Lengths beyond the limit are refused from the fixed header alone.
        let mut huge = [0u8; 16];
        huge[..4].copy_from_slice(&[b'l', 1, 0, 1]);
        huge[4..8].copy_from_slice(&(MAX_MESSAGE as u32).to_le_bytes());
        assert!(message_length(&huge).is_err());
        // A message that says it carries descriptors (header fields written by hand).
        let mut w = Writer { buf: vec![b'l', 1, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0] };
        w.value(&Value::Array {
            element: "(yv)".into(),
            items: vec![
                Value::Struct(vec![Value::Byte(1), Value::Variant(Box::new(Value::Path("/a".into())))]),
                Value::Struct(vec![Value::Byte(3), Value::Variant(Box::new(Value::Str("C".into())))]),
                Value::Struct(vec![Value::Byte(9), Value::Variant(Box::new(Value::U32(1)))]),
            ],
        })
        .unwrap();
        w.align(8);
        assert!(Message::parse(&w.buf).unwrap_err().to_string().contains("descriptors"));
    }

    #[test]
    fn signatures_follow_the_grammar() {
        for good in ["", "s", "a{sv}", "(ii)", "aa{s(yv)}", "a(oa{sv})", "v", "ay"] {
            assert!(parse_signature(good).is_ok(), "{good}");
        }
        for bad_sig in ["(", ")", "()", "a", "{sv}", "a{vs}", "a{s}", "a{sss}", "z", "(i", "a{sv"] {
            assert!(parse_signature(bad_sig).is_err(), "{bad_sig}");
        }
        assert!(parse_signature(&format!("{}y", "a".repeat(32))).is_ok());
        assert!(parse_signature(&format!("{}y", "a".repeat(33))).is_err());
        assert!(parse_signature(&format!("{}y{}", "(".repeat(33), ")".repeat(33))).is_err());
        assert!(parse_signature(&"y".repeat(256)).is_err());
    }

    #[test]
    fn names_and_paths() {
        assert!(is_bus_name("org.mpris.MediaPlayer2.spotify") && is_bus_name(":1.42") && is_bus_name("a-b.c"));
        assert!(!is_bus_name("spotify") && !is_bus_name("org..x") && !is_bus_name("1org.x") && !is_bus_name(""));
        assert!(is_interface_name("org.mpris.MediaPlayer2.Player") && !is_interface_name("org.mpris-x.A"));
        assert!(is_member_name("PropertiesChanged") && !is_member_name("Get.All") && !is_member_name("9x"));
        assert!(is_object_path("/") && is_object_path("/org/mpris/MediaPlayer2"));
        for bad_path in ["", "org", "/org/", "//", "/a//b", "/a-b", "/ä"] {
            assert!(!is_object_path(bad_path), "{bad_path}");
        }
    }

    #[test]
    fn deep_nesting_and_value_floods_are_bounded() {
        // Variants inside variants, past the depth limit.
        let mut v = Value::U32(1);
        for _ in 0..(MAX_DEPTH + 2) {
            v = Value::Variant(Box::new(v));
        }
        let mut m = Message::call(":1.1", "/a", "a.b", "C", vec![v]);
        m.serial = 1;
        let bytes = m.encode().unwrap();
        assert!(Message::parse(&bytes).unwrap_err().to_string().contains("deep"));
        // An array of many small values.
        let items = vec![Value::U32(0); MAX_VALUES + 1];
        let mut m = Message::call(":1.1", "/a", "a.b", "C", vec![Value::Array { element: "u".into(), items }]);
        m.serial = 1;
        let bytes = m.encode().unwrap();
        assert!(Message::parse(&bytes).unwrap_err().to_string().contains("too many values"));
    }

    #[test]
    fn the_fuzz_harness_accepts_its_own_seeds() {
        let mut m =
            Message::call(":1.1", "/a", "a.b", "C", vec![Value::string_dict(vec![("k".into(), Value::Bool(true))])]);
        m.serial = 9;
        fuzz(&m.encode().unwrap());
        fuzz(b"");
        fuzz(&[b'l'; 40]);
    }
}
