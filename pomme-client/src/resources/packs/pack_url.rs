//! Vanilla `ClientCommonPacketListenerImpl.parseResourcePackUrl`, which asks
//! `java.net.URL` whether a server's pack URL parses: a port of the checks
//! JDK 25 makes for an http or https URL.

/// Whether `new URL` takes `url` with an http or https scheme. One it takes
/// but can't fetch fails its download instead.
pub(crate) fn is_pack_url(url: &str) -> bool {
    // `URL(String)`: surrounding whitespace and a `url:` prefix are skipped.
    let spec = url.trim_matches(|c: char| c <= ' ');
    let spec = match spec.get(..4) {
        Some(prefix) if prefix.eq_ignore_ascii_case("url:") => &spec[4..],
        _ => spec,
    };
    let Some((scheme, rest)) = spec.split_once(':') else {
        return false;
    };
    (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        && authority_parses(rest.split('#').next().unwrap_or_default())
}

/// `URLStreamHandler.parseURL`'s authority: an IPv6 literal must be one, a
/// port a number of at least -1, and the parts it hands `setURL` must pass
/// `IPAddressUtil`'s character checks.
fn authority_parses(rest: &str) -> bool {
    let Some(rest) = rest.strip_prefix("//") else {
        return true;
    };
    // The query is cut off first, so a `?` ends it as well as a `/`.
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    // Two `@`s leave no user info and an empty host.
    let (user_info, host) = match authority.split_once('@') {
        None => (None, authority),
        Some((user_info, host)) if !host.contains('@') => (Some(user_info), host),
        Some(_) => (None, ""),
    };
    let (host_passes, port) = if let Some(literal) = host.strip_prefix('[') {
        let Some(end) = literal.find(']').filter(|&end| end > 1) else {
            return false;
        };
        let address = &literal[..end];
        let port = match &literal[end + 1..] {
            "" => None,
            after => match after.strip_prefix(':') {
                Some(port) => Some(port),
                None => return false,
            },
        };
        // `checkHost`: an IPv6 literal's scope, from its `%`.
        let scope_passes = address
            .find('%')
            .is_none_or(|scope| passes(&address[scope..], SCOPE_REJECTS));
        (is_ipv6_literal(address) && scope_passes, port)
    } else {
        let (host, port) = host
            .split_once(':')
            .map_or((host, None), |(host, port)| (host, Some(port)));
        (passes(host, HOST_REJECTS), port)
    };
    // An empty port is none.
    let port_parses = port
        .filter(|port| !port.is_empty())
        .is_none_or(|port| parse_int(port).is_some_and(|port| port >= -1));
    host_passes
        && port_parses
        && user_info.is_none_or(|user_info| passes(user_info, USER_INFO_REJECTS))
        && passes(authority, AUTHORITY_REJECTS)
}

/// UTF-16 units one of `IPAddressUtil`'s checks rejects, as inclusive
/// ranges; JDK 25 lists them, with `jdk.net.url.delayParsing` off.
type Rejects = &'static [(u32, u32)];

/// `checkHost` for a host that isn't an IPv6 literal.
const HOST_REJECTS: Rejects = &[
    (0x0000, 0x0020),
    (0x0022, 0x0023),
    (0x002F, 0x002F),
    (0x003A, 0x003A),
    (0x003C, 0x003C),
    (0x003E, 0x0040),
    (0x005B, 0x005E),
    (0x0060, 0x0060),
    (0x007B, 0x007D),
    (0x007F, 0x007F),
    (0x2047, 0x2049),
    (0x2100, 0x2101),
    (0x2105, 0x2106),
    (0x2A74, 0x2A74),
    (0xFE55, 0xFE56),
    (0xFE5F, 0xFE5F),
    (0xFE6B, 0xFE6B),
    (0xFF03, 0xFF03),
    (0xFF0F, 0xFF0F),
    (0xFF1A, 0xFF1A),
    (0xFF1F, 0xFF20),
];
/// `checkHost` for an IPv6 literal's scope.
const SCOPE_REJECTS: Rejects = &[
    (0x0000, 0x001F),
    (0x0023, 0x0023),
    (0x002F, 0x002F),
    (0x003F, 0x003F),
    (0x005B, 0x005D),
    (0x007F, 0x007F),
];
/// `checkUserInfo`.
const USER_INFO_REJECTS: Rejects = &[
    (0x0000, 0x001F),
    (0x0023, 0x0023),
    (0x002F, 0x002F),
    (0x003F, 0x0040),
    (0x005B, 0x005D),
    (0x007F, 0x007F),
];
/// `checkAuth`.
const AUTHORITY_REJECTS: Rejects = &[
    (0x0000, 0x001F),
    (0x0023, 0x0023),
    (0x002F, 0x002F),
    (0x003F, 0x003F),
    (0x005C, 0x005C),
    (0x007F, 0x007F),
];

/// `IPAddressUtil.scan` finding nothing.
fn passes(s: &str, rejects: Rejects) -> bool {
    s.chars().all(|c| {
        let c = u32::from(c);
        !rejects
            .iter()
            .any(|&(first, last)| (first..=last).contains(&c))
    })
}

/// `IPAddressUtil.textToNumericFormatV6` succeeding, with its ASCII digits.
fn is_ipv6_literal(src: &str) -> bool {
    const INADDR16SZ: usize = 16;
    const INT16SZ: usize = 2;
    const INADDR4SZ: usize = 4;

    let src: Vec<char> = src.chars().collect();
    if src.len() < 2 {
        return false;
    }
    let mut len = src.len();
    if let Some(pc) = src.iter().position(|&c| c == '%') {
        if pc == len - 1 {
            return false;
        }
        len = pc;
    }
    let mut i = 0;
    if src[0] == ':' {
        i = 1;
        if src[1] != ':' {
            return false;
        }
    }
    let mut colonp = None;
    let mut j = 0;
    let mut curtok = i;
    let mut saw_xdigit = false;
    let mut val = 0u32;
    while i < len {
        let ch = src[i];
        i += 1;
        if let Some(digit) = ch.to_digit(16) {
            val = (val << 4) | digit;
            if val > 0xffff {
                return false;
            }
            saw_xdigit = true;
            continue;
        }
        if ch == ':' {
            curtok = i;
            if !saw_xdigit {
                if colonp.is_some() {
                    return false;
                }
                colonp = Some(j);
                continue;
            } else if i == len || j + INT16SZ > INADDR16SZ {
                return false;
            }
            j += INT16SZ;
            saw_xdigit = false;
            val = 0;
            continue;
        }
        if ch == '.' && j + INADDR4SZ <= INADDR16SZ {
            let ia4: String = src[curtok..len].iter().collect();
            if ia4.matches('.').count() != 3 || !is_ipv4_literal(&ia4) {
                return false;
            }
            j += INADDR4SZ;
            saw_xdigit = false;
            break;
        }
        return false;
    }
    if saw_xdigit {
        if j + INT16SZ > INADDR16SZ {
            return false;
        }
        j += INT16SZ;
    }
    if colonp.is_some() {
        if j == INADDR16SZ {
            return false;
        }
        j = INADDR16SZ;
    }
    j == INADDR16SZ
}

/// `IPAddressUtil.textToNumericFormatV4` succeeding: up to four parts, the
/// last filling the bytes left, leading zeros allowed.
fn is_ipv4_literal(src: &str) -> bool {
    let len = src.chars().count();
    if len == 0 || len > 15 {
        return false;
    }
    let mut value = 0u64;
    let mut octet = 0;
    let mut new_octet = true;
    for c in src.chars() {
        if c == '.' {
            if new_octet || value > 0xff || octet == 3 {
                return false;
            }
            octet += 1;
            value = 0;
            new_octet = true;
        } else {
            let Some(digit) = c.to_digit(10) else {
                return false;
            };
            value = value * 10 + u64::from(digit);
            new_octet = false;
        }
    }
    !new_octet && value < 1 << ((4 - octet) * 8)
}

/// `Integer.parseInt(s, 10)`: an optional sign, then `Character.digit`'s
/// decimal digits, in any script.
fn parse_int(s: &str) -> Option<i32> {
    let (negative, digits) = match s.strip_prefix('-') {
        Some(digits) => (true, digits),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    if digits.is_empty() {
        return None;
    }
    let mut value = 0i64;
    for c in digits.chars() {
        value = value * 10 + i64::from(decimal_digit(c)?);
        if value > i64::from(i32::MAX) + 1 {
            return None;
        }
    }
    i32::try_from(if negative { -value } else { value }).ok()
}

/// The first of each run of ten UTF-16 units `Character.digit(c, 10)` reads
/// as 0 to 9, as JDK 25 lists them.
const DECIMAL_ZEROS: [u32; 37] = [
    0x0030, 0x0660, 0x06F0, 0x07C0, 0x0966, 0x09E6, 0x0A66, 0x0AE6, 0x0B66, 0x0BE6, 0x0C66, 0x0CE6,
    0x0D66, 0x0DE6, 0x0E50, 0x0ED0, 0x0F20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80,
    0x1A90, 0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0,
    0xFF10,
];

fn decimal_digit(c: char) -> Option<u32> {
    let c = u32::from(c);
    DECIMAL_ZEROS
        .iter()
        .find(|&&zero| (zero..zero + 10).contains(&c))
        .map(|zero| c - zero)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `java.net.URL` on JDK 25 takes for an http or https URL, each case
    /// checked against it.
    #[test]
    fn pack_urls_parse_as_java_does() {
        for good in [
            "https://example.com/pack.zip",
            "http://127.0.0.1:8080/pack.zip",
            "HTTP://example.com/pack.zip",
            "http://",
            " https://example.com/pack.zip ",
            "URL:https://example.com/pack.zip",
            "http://example.com:/pack.zip",
            "http://[::1]:25565/pack.zip",
            "http://a@b@c/pack.zip",
            "http://example.com/a:b",
            "http://example.com?a:b/c",
            "http://[::1.2.3.04]/p",
            "http://[::01.2.3.4]/p",
            "http://[::ffff:01.2.3.4]/p",
            "http://[00001::]/p",
            "http://h:\u{0668}\u{0660}/p",
            "http://h:\u{FF18}\u{FF10}/p",
            "http://h:+80/p",
            "http://[fe80::1%25eth0]/p",
            "http://h~!$/p",
            "http://user:pw@h/p",
        ] {
            assert!(is_pack_url(good), "{good}");
        }
        for bad in [
            "",
            "not a url",
            "ftp://example.com/pack.zip",
            "file:///pack.zip",
            "http://example.com:abc/pack.zip",
            "http://example.com:-5/pack.zip",
            "http://[::1/pack.zip",
            "http://[nope]/pack.zip",
            "http://[::1]x/pack.zip",
            "http://[::1%]/p",
            "http://[fe80::1%]/p",
            "http://h:2147483648/p",
            "http://exa mple.com/p",
            "http://a\\b/p",
            "http://h\u{FF03}x/p",
            "http://u\u{0001}@h/p",
            "http://[::1%a#b]/p",
        ] {
            assert!(!is_pack_url(bad), "{bad}");
        }
    }
}
