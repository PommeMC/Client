//! A server's address as typed, and vanilla's rules for reading one
//! (`ServerAddress.parseString`, over Guava's `HostAndPort.fromString`).

use std::fmt;

/// `ServerAddress`'s default port.
pub const DEFAULT_PORT: u16 = 25565;

/// A host and port, not yet resolved.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ServerAddr {
    pub host: String,
    pub port: u16,
}

impl ServerAddr {
    /// `host`, `host:port`, `[v6]` or `[v6]:port`; the port defaults to
    /// 25565. A host with more than one colon and no brackets is all host.
    /// `None` where vanilla falls back to its invalid address: an empty host,
    /// a port that isn't a number in range, or misplaced brackets.
    pub fn parse(input: &str) -> Option<Self> {
        let (host, port) = match input.strip_prefix('[') {
            Some(rest) => {
                let (host, after) = rest.rsplit_once(']')?;
                // Guava takes the brackets for an IPv6 literal's.
                if !host.contains(':') {
                    return None;
                }
                match after.strip_prefix(':') {
                    // Digits only here; a bare `host:port` may carry a sign.
                    Some(port) if port.bytes().all(|b| b.is_ascii_digit()) => (host, port),
                    Some(_) => return None,
                    None if after.is_empty() => (host, ""),
                    None => return None,
                }
            }
            None => match input.split_once(':') {
                Some((host, port)) if !port.contains(':') => (host, port),
                _ => (input, ""),
            },
        };
        let port = match port {
            "" => DEFAULT_PORT,
            // `Ints.tryParse` takes a minus sign but no plus, then the value
            // has to be a port.
            port if port.starts_with('+') => return None,
            port => u16::try_from(port.parse::<i32>().ok()?).ok()?,
        };
        (!host.is_empty()).then(|| Self {
            host: host.to_owned(),
            port,
        })
    }
}

impl fmt::Display for ServerAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Option<(String, u16)> {
        ServerAddr::parse(input).map(|addr| (addr.host, addr.port))
    }

    fn addr(host: &str, port: u16) -> Option<(String, u16)> {
        Some((host.to_owned(), port))
    }

    #[test]
    fn the_port_defaults_to_25565() {
        assert_eq!(parse("play.example.org"), addr("play.example.org", 25565));
        assert_eq!(parse("play.example.org:"), addr("play.example.org", 25565));
        assert_eq!(
            parse("play.example.org:25570"),
            addr("play.example.org", 25570)
        );
        assert_eq!(parse("127.0.0.1:1"), addr("127.0.0.1", 1));
        assert_eq!(parse("host:-0"), addr("host", 0));
    }

    #[test]
    fn ipv6_takes_brackets_to_carry_a_port() {
        assert_eq!(parse("[::1]"), addr("::1", 25565));
        assert_eq!(parse("[::1]:25570"), addr("::1", 25570));
        // Without brackets the colons are the host's.
        assert_eq!(parse("::1"), addr("::1", 25565));
        assert_eq!(parse("2001:db8::1"), addr("2001:db8::1", 25565));
    }

    #[test]
    fn what_vanilla_calls_invalid_is_none() {
        for input in [
            "",
            ":25565",
            "host:port",
            "host:65536",
            "host:99999999999",
            "host:-1",
            "host:+25565",
            "host: 25565",
            "[::1",
            "[::1]25565",
            "[host]:25565",
            "[::1]:port",
            "[::1]:-0",
        ] {
            assert_eq!(parse(input), None, "{input:?}");
        }
    }

    #[test]
    fn it_displays_as_host_and_port() {
        let addr = ServerAddr::parse("play.example.org").unwrap();
        assert_eq!(addr.to_string(), "play.example.org:25565");
    }
}
