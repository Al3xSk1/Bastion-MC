//! The recorder's replay buffer, and what saving a clip takes from it.
//!
//! The recorder (an ffmpeg in a sandbox of its own) writes one-second MPEG transport stream
//! segments into the buffer directory, reusing a fixed set of names (the segment muxer's
//! wrap), and a list of the newest ones (`list.csv`: `name,start,end` per line). Saving
//! needs no program: transport-stream segments joined end to end are a transport stream.
//! The launcher reads the list, opens the newest segments it needs — by name relative to
//! the directory, refusing links and anything not a regular file — and concatenates them,
//! renumbering each stream's continuity counter so that the result is one clean stream
//! (players otherwise report a discontinuity at every joint).
//!
//! Everything in that directory was written by the recorder, which processes what the game
//! draws: untrusted. The list is parsed strictly ([`parse_list`]); segments are bounded in
//! size and must be whole transport-stream packets ([`Joiner`]); [`fuzz`] runs both on
//! arbitrary bytes. The bytes of a clip are otherwise passed through as the recorder made
//! them: a player that opens the clip parses them.

use std::{fs::File, io::Read, os::fd::AsFd};

/// The segment list's name in the buffer directory.
pub const LIST: &str = "list.csv";
/// The recorder's latest thumbnail.
pub const THUMB: &str = "thumb.jpg";
pub const MAX_LIST_BYTES: u64 = 64 * 1024;
/// One segment (a second of video, normally a few hundred kilobytes).
pub const MAX_SEGMENT_BYTES: u64 = 64 << 20;
/// One clip.
pub const MAX_CLIP_BYTES: u64 = 512 << 20;
const PACKET: usize = 188;
const SYNC: u8 = 0x47;
const MAX_LIST_LINES: usize = 1000;

/// One entry of the list: a segment file and the time it covers (seconds, stream time).
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub name: String,
    pub start: f64,
    pub end: f64,
}

impl Segment {
    pub fn seconds(&self) -> f64 {
        self.end - self.start
    }
}

/// `r<run>_<three digits>.ts`, the names the recorder gives segments.
pub fn is_segment_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('r').and_then(|r| r.strip_suffix(".ts")) else { return false };
    let Some((run, index)) = rest.split_once('_') else { return false };
    (1..=6).contains(&run.len())
        && run.bytes().all(|b| b.is_ascii_digit())
        && index.len() == 3
        && index.bytes().all(|b| b.is_ascii_digit())
}

/// A time in the list: digits, optionally a point and more digits. No signs, no exponents.
fn time(text: &str) -> Option<f64> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let digits = |s: &str, max: usize| s.len() <= max && s.bytes().all(|b| b.is_ascii_digit());
    if whole.is_empty() || !digits(whole, 9) || !digits(fraction, 9) {
        return None;
    }
    text.parse::<f64>().ok().filter(|t| t.is_finite())
}

/// The list as the segment muxer writes it. A last line without its newline is still being
/// written and is left out; any complete line that is not `name,start,end` (with a segment
/// name, start before end, at most a minute apart) makes the whole list unusable.
pub fn parse_list(raw: &[u8]) -> Result<Vec<Segment>, String> {
    if raw.len() as u64 > MAX_LIST_BYTES {
        return Err("the segment list is too large".into());
    }
    let text = std::str::from_utf8(raw).map_err(|_| "the segment list is not text".to_string())?;
    let complete = match text.rfind('\n') {
        Some(end) => &text[..end],
        None => "",
    };
    let mut segments = Vec::new();
    for line in complete.split('\n').filter(|l| !l.is_empty()) {
        if segments.len() == MAX_LIST_LINES {
            return Err("the segment list is too long".into());
        }
        let mut fields = line.split(',');
        let (Some(name), Some(start), Some(end), None) = (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err("a malformed line in the segment list".into());
        };
        let (Some(start), Some(end)) = (time(start), time(end)) else {
            return Err("a malformed time in the segment list".into());
        };
        if !is_segment_name(name) || end <= start || end - start > 60.0 {
            return Err("an implausible entry in the segment list".into());
        }
        segments.push(Segment { name: name.to_string(), start, end });
    }
    // Each segment starts where the one before it ended (one recording run).
    for pair in segments.windows(2) {
        if (pair[1].start - pair[0].end).abs() > 0.5 {
            return Err("the segment list has a gap".into());
        }
    }
    Ok(segments)
}

/// The newest segments that together cover `seconds` (all of them, if they do not).
pub fn pick(list: &[Segment], seconds: f64) -> &[Segment] {
    let mut covered = 0.0;
    let mut first = list.len();
    while first > 0 && covered < seconds {
        first -= 1;
        covered += list[first].seconds();
    }
    &list[first..]
}

/// Renumbers continuity counters across joined segments: each stream's packets carrying a
/// payload count on from the last one written, so the joints are not discontinuities.
pub struct Joiner {
    counters: Vec<Option<u8>>,
    pub packets: u64,
}

impl Default for Joiner {
    fn default() -> Self {
        Joiner { counters: vec![None; 8192], packets: 0 }
    }
}

impl Joiner {
    /// Rewrite `data` (whole packets) in place.
    pub fn feed(&mut self, data: &mut [u8]) -> Result<(), String> {
        if !data.len().is_multiple_of(PACKET) {
            return Err("a segment is not made of whole transport-stream packets".into());
        }
        for packet in data.as_chunks_mut::<PACKET>().0 {
            if packet[0] != SYNC {
                return Err("a segment lost transport-stream sync".into());
            }
            let pid = usize::from(packet[1] & 0x1f) << 8 | usize::from(packet[2]);
            let payload = packet[3] & 0x10 != 0;
            // Null packets carry nothing to count; packets without payload keep their counter.
            if pid != 0x1fff && payload {
                let next = match self.counters[pid] {
                    Some(last) => (last + 1) & 0x0f,
                    None => packet[3] & 0x0f,
                };
                packet[3] = (packet[3] & 0xf0) | next;
                self.counters[pid] = Some(next);
            }
            self.packets += 1;
        }
        Ok(())
    }
}

/// Read one segment of the list from the open buffer directory: a regular file reached
/// without following a link, non-empty, bounded, whole packets.
pub fn read_segment(dir: impl AsFd, name: &str) -> Result<Vec<u8>, String> {
    if !is_segment_name(name) {
        return Err("not a segment".into());
    }
    let file = crate::util::openat_regular_nofollow(dir, std::ffi::OsStr::new(name))
        .map_err(|e| format!("a segment of the buffer cannot be opened ({e})"))?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    if size == 0 || size > MAX_SEGMENT_BYTES || !size.is_multiple_of(PACKET as u64) {
        return Err("a segment of the buffer is being rewritten; try again".into());
    }
    let mut data = Vec::with_capacity(size as usize);
    file.take(MAX_SEGMENT_BYTES).read_to_end(&mut data).map_err(|e| e.to_string())?;
    if data.len() as u64 != size {
        return Err("a segment of the buffer changed while it was read; try again".into());
    }
    Ok(data)
}

/// Read the list from the open buffer directory.
pub fn read_list(dir: impl AsFd) -> Result<Vec<Segment>, String> {
    let file = crate::util::openat_regular_nofollow(dir, std::ffi::OsStr::new(LIST))
        .map_err(|_| "nothing has been recorded yet".to_string())?;
    let mut raw = Vec::new();
    file.take(MAX_LIST_BYTES + 1).read_to_end(&mut raw).map_err(|e| e.to_string())?;
    parse_list(&raw)
}

/// Join `segments` (from the open buffer directory) into `out`; returns the bytes written.
pub fn join(dir: &File, segments: &[Segment], out: &mut impl std::io::Write) -> Result<u64, String> {
    let mut joiner = Joiner::default();
    let mut total = 0u64;
    for segment in segments {
        let mut data = read_segment(dir, &segment.name)?;
        joiner.feed(&mut data)?;
        total += data.len() as u64;
        if total > MAX_CLIP_BYTES {
            return Err("the clip would be larger than a clip may be".into());
        }
        out.write_all(&data).map_err(|e| format!("writing the clip: {e}"))?;
    }
    if total == 0 {
        return Err("nothing has been recorded yet".into());
    }
    Ok(total)
}

/// The list parser and the joiner on arbitrary bytes.
#[doc(hidden)]
pub fn fuzz(data: &[u8]) {
    if let Ok(list) = parse_list(data) {
        for s in &list {
            assert!(is_segment_name(&s.name) && s.start < s.end && s.end - s.start <= 60.0);
        }
        let picked = pick(&list, 10.0);
        assert!(picked.len() <= list.len());
    }
    let mut packets = data[..data.len() - data.len() % PACKET].to_vec();
    let mut joiner = Joiner::default();
    if joiner.feed(&mut packets).is_ok() {
        // Sync bytes and stream ids are never touched; only counters change.
        for (a, b) in packets.as_chunks::<PACKET>().0.iter().zip(data.as_chunks::<PACKET>().0) {
            assert_eq!((a[0], a[1], a[2], a[3] & 0xf0), (b[0], b[1], b[2], b[3] & 0xf0));
            assert_eq!(a[4..], b[4..]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_is_read_as_the_segment_muxer_writes_it() {
        let raw =
            b"r1_004.ts,4.000000,5.000000\nr1_000.ts,5.000000,6.000000\nr1_001.ts,6.000000,7.000000\nr1_002.ts,7.0";
        let list = parse_list(raw).unwrap();
        assert_eq!(list.len(), 3, "the last line is still being written");
        assert_eq!(list[0], Segment { name: "r1_004.ts".into(), start: 4.0, end: 5.0 });
        assert_eq!(pick(&list, 2.0).iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["r1_000.ts", "r1_001.ts"]);
        assert_eq!(pick(&list, 1.5).len(), 2);
        assert_eq!(pick(&list, 99.0).len(), 3);
        assert!(parse_list(b"").unwrap().is_empty());
        for bad in [
            &b"../../etc/passwd,0.0,1.0\n"[..],
            b"r1_000.ts,1.0,0.5\n",
            b"r1_000.ts,0.0,100.0\n",
            b"r1_000.ts,-1.0,1.0\n",
            b"r1_000.ts,1e3,2e3\n",
            b"r1_000.ts,0.0\n",
            b"r1_000.ts,0.0,1.0,x\n",
            b"r1_000.ts,0.0,1.0\nr1_001.ts,5.0,6.0\n",
            b"r1_0000.ts,0.0,1.0\n",
            b"r_000.ts,0.0,1.0\n",
            b"r1_000.mp4,0.0,1.0\n",
            b"r1_000.ts,NaN,1.0\n",
        ] {
            assert!(parse_list(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    fn packet(pid: u16, cc: u8, payload: bool) -> Vec<u8> {
        let mut p = vec![0xffu8; PACKET];
        p[0] = SYNC;
        p[1] = (pid >> 8) as u8 & 0x1f;
        p[2] = pid as u8;
        p[3] = if payload { 0x10 } else { 0x20 } | cc;
        p
    }

    #[test]
    fn joints_get_counters_that_continue() {
        let mut joiner = Joiner::default();
        let mut first =
            [packet(0, 0, true), packet(256, 0, true), packet(256, 1, true), packet(256, 1, false)].concat();
        joiner.feed(&mut first).unwrap();
        // The second segment starts its counters at 0 again.
        let mut second = [packet(0, 0, true), packet(256, 0, true), packet(0x1fff, 3, true)].concat();
        joiner.feed(&mut second).unwrap();
        let counters: Vec<u8> = second.as_chunks::<PACKET>().0.iter().map(|p| p[3] & 0x0f).collect();
        assert_eq!(counters, [1, 2, 3], "PAT 0→1, video 1→2, a null packet untouched");
        assert_eq!(first[3 * PACKET + 3] & 0x0f, 1, "a packet without payload keeps its counter");
        assert_eq!(joiner.packets, 7);
        let mut broken = packet(0, 0, true);
        broken[0] = 0;
        assert!(joiner.feed(&mut broken).is_err());
        assert!(joiner.feed(&mut [SYNC; 100]).is_err());
    }
}
