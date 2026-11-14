//! The Spotify URIs the phone may name: `spotify:<kind>:<id>` for the six documented kinds,
//! the id being Spotify's 22-character base-62 identifier. Nothing else is ever opened,
//! queued or put into a request — not a URL, not a local file, not a user or a collection.

/// What a URI points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Track,
    Album,
    Playlist,
    Artist,
    Episode,
    Show,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Track => "track",
            Kind::Album => "album",
            Kind::Playlist => "playlist",
            Kind::Artist => "artist",
            Kind::Episode => "episode",
            Kind::Show => "show",
        }
    }

    /// Played on its own (a track, an episode) rather than as a context (an album, …).
    pub fn is_item(self) -> bool {
        matches!(self, Kind::Track | Kind::Episode)
    }
}

/// A checked Spotify URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uri {
    pub kind: Kind,
    pub id: String,
}

impl Uri {
    /// `spotify:<kind>:<22 base-62 characters>`, exactly.
    pub fn parse(text: &str) -> Option<Uri> {
        let mut parts = text.split(':');
        let (scheme, kind, id) = (parts.next()?, parts.next()?, parts.next()?);
        if scheme != "spotify" || parts.next().is_some() {
            return None;
        }
        let kind = match kind {
            "track" => Kind::Track,
            "album" => Kind::Album,
            "playlist" => Kind::Playlist,
            "artist" => Kind::Artist,
            "episode" => Kind::Episode,
            "show" => Kind::Show,
            _ => return None,
        };
        (id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric())).then(|| Uri { kind, id: id.to_string() })
    }

    pub fn text(&self) -> String {
        format!("spotify:{}:{}", self.kind.word(), self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_documented_kinds_with_a_base62_id() {
        for (text, kind) in [
            ("spotify:track:4uLU6hMCjMI75M1A2tKUQC", Kind::Track),
            ("spotify:album:2noRn2Aes5aoNVsU6iWThc", Kind::Album),
            ("spotify:playlist:37i9dQZF1DXcBWIGoYBM5M", Kind::Playlist),
            ("spotify:artist:4tZwfgrHOc3mvqYlEYSvVi", Kind::Artist),
            ("spotify:episode:512ojhOuo1ktJprKbVcKyQ", Kind::Episode),
            ("spotify:show:5CfCWKI5pZ28U0uOzXkDHe", Kind::Show),
        ] {
            let uri = Uri::parse(text).unwrap();
            assert_eq!((uri.kind, uri.text()), (kind, text.to_string()));
        }
        for bad in [
            "",
            "spotify:track:4uLU6hMCjMI75M1A2tKUQ",   // 21
            "spotify:track:4uLU6hMCjMI75M1A2tKUQCC", // 23
            "spotify:track:4uLU6hMCjMI75M1A2tKUQ-",
            "spotify:user:alice",
            "spotify:user:alice:playlist:37i9dQZF1DXcBWIGoYBM5M",
            "spotify:local:a:b:c:1",
            "spotify:collection:tracks",
            "https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC",
            "SPOTIFY:track:4uLU6hMCjMI75M1A2tKUQC",
            "spotify:Track:4uLU6hMCjMI75M1A2tKUQC",
            "spotify:track:4uLU6hMCjMI75M1A2tKUQC:x",
            "spotify:track:4uLU6hMCjMI75M1A2tKUQ\u{0}",
        ] {
            assert_eq!(Uri::parse(bad), None, "{bad:?}");
        }
    }
}
