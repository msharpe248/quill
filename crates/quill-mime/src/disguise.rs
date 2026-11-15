//! Links that say one place and go to another.
//!
//! "www.mybank.example" written over a link to somewhere else is the oldest
//! trick in phishing. When a link's text itself looks like an address, the
//! reader is told where the link really goes.
//!
//! The rule errs towards telling: a warning on an honest link costs a line,
//! and a missed one costs an account. Text that names no address ("click
//! here") is never flagged, and neither is text that names the place the
//! link goes or the site that place is part of.

/// The longest a host or an address is shown before it is cut.
const MAX_SHOWN: usize = 60;

/// A link whose text names somewhere it does not go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Disguise {
    /// Where the link really goes: a host, a mail address, or the whole
    /// target when no host can be read out of it.
    pub real: String,
    /// What the text claims, when that can be said briefly.
    pub shown: Option<String>,
}

impl Disguise {
    /// The warning, for a link numbered `number`.
    pub(crate) fn warning(&self, number: usize) -> String {
        match &self.shown {
            Some(shown) => format!("! [{number}] goes to {}, not {shown}", self.real),
            None => format!("! [{number}] goes to {}", self.real),
        }
    }
}

/// Compares what a link shows (`text`) with where it leads (`href`).
pub(crate) fn find(text: &str, href: &str) -> Option<Disguise> {
    let target = Target::of(href);
    // Invisible characters cannot be allowed to split a name in two.
    let visible: String = text.chars().filter(|c| !is_hidden(*c)).collect();
    visible
        .split_whitespace()
        .filter_map(|word| Claim::of(strip_punctuation(word)))
        .find_map(|claim| judge(&claim, &target, href))
}

/// What a piece of link text claims to be.
#[derive(Debug, PartialEq, Eq)]
enum Claim {
    /// An address on this host.
    Host(String),
    /// A mail address.
    Mail(String),
    /// An address written so that its host cannot be trusted, such as one
    /// with a user name before an `@`.
    Unclear,
}

/// Where a link leads.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    Host(String),
    Mail(String),
    /// Something with no host that can be read out of it for certain.
    Other,
}

fn judge(claim: &Claim, target: &Target, href: &str) -> Option<Disguise> {
    let disguise = |real: &str, shown: Option<&str>| Disguise {
        real: shorten(real),
        shown: shown.map(shorten),
    };
    match (claim, target) {
        (Claim::Mail(shown), Target::Mail(real)) => {
            (shown != real).then(|| disguise(real, Some(shown)))
        }
        (Claim::Mail(shown), Target::Host(real)) => {
            let shown = domain(shown);
            (!same_site(shown, real)).then(|| disguise(real, Some(shown)))
        }
        (Claim::Host(shown), Target::Host(real)) => {
            (!same_site(shown, real)).then(|| disguise(real, Some(shown)))
        }
        (Claim::Host(shown), Target::Mail(real)) => {
            (!same_site(shown, domain(real))).then(|| disguise(real, Some(shown)))
        }
        (Claim::Unclear, Target::Host(real) | Target::Mail(real)) => Some(disguise(real, None)),
        (Claim::Host(shown) | Claim::Mail(shown), Target::Other) => {
            Some(disguise(href.trim(), Some(shown)))
        }
        (Claim::Unclear, Target::Other) => Some(disguise(href.trim(), None)),
    }
}

impl Claim {
    fn of(word: &str) -> Option<Self> {
        let lower = word.to_lowercase();
        if let Some(address) = lower.strip_prefix("mailto:") {
            let address = address.split('?').next().unwrap_or_default();
            return Some(match is_mail_address(address) {
                true => Self::Mail(address.to_owned()),
                false => Self::Unclear,
            });
        }
        if let Some((scheme, rest)) = lower.split_once("://") {
            let is_scheme = scheme.chars().next().is_some_and(|c| c.is_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '+' | '-' | '.'));
            if !is_scheme {
                return None;
            }
            let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
            let host = without_port(authority);
            return Some(match is_host_name(host) || is_ip_address(host) {
                true => Self::Host(tidy(host)),
                // A user name, a backslash, an escape: whatever it is, the
                // reader cannot tell where it claims to go.
                false => Self::Unclear,
            });
        }
        if is_mail_address(&lower) {
            return Some(Self::Mail(lower));
        }

        // A bare name: "www.mybank.example", "mybank.example/login".
        let end = lower.find(['/', '?', '#']).unwrap_or(lower.len());
        let (authority, path) = lower.split_at(end);
        if authority.contains('@') {
            // "mybank.example@evil.example/": a user name dressed as a host.
            return is_host_name(domain(authority)).then_some(Self::Unclear);
        }
        let host = without_port(authority);
        if is_ip_address(host) {
            return Some(Self::Host(tidy(host)));
        }
        if !is_host_name(host) {
            return None;
        }
        // "report.pdf" and "index.html" are files, not hosts, unless
        // something else about the word says address.
        let ending = host.rsplit('.').next().unwrap_or_default();
        let says_address =
            !path.is_empty() || host.starts_with("www.") || host.len() != authority.len();
        if !says_address && FILE_ENDINGS.contains(&ending) {
            return None;
        }
        Some(Self::Host(tidy(host)))
    }
}

impl Target {
    fn of(href: &str) -> Self {
        let href = href.trim();
        let lower = href.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("mailto:") {
            let address = rest.split(['?', ',']).next().unwrap_or_default();
            return match is_mail_address(address) {
                true => Self::Mail(address.to_owned()),
                false => Self::Other,
            };
        }
        let Some(rest) = lower
            .strip_prefix("https://")
            .or_else(|| lower.strip_prefix("http://"))
        else {
            return Self::Other;
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        // Programs disagree about where a host ends once a backslash is
        // involved, so nothing is read out of one.
        if authority.contains('\\') {
            return Self::Other;
        }
        // What comes before the last `@` is a user name, whatever it looks
        // like.
        let host = authority.rsplit('@').next().unwrap_or_default();
        match ascii_host(without_port(host)) {
            Some(host) => Self::Host(host),
            None => Self::Other,
        }
    }
}

/// A host as it is looked up: plain ASCII, with every label in another
/// script in its `xn--` form. A name spelt with a lookalike letter shows
/// for what it is that way. `None` for anything that is not a host name.
fn ascii_host(host: &str) -> Option<String> {
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty() || host.len() > MAX_HOST_BYTES {
        return None;
    }
    let mut ascii = String::with_capacity(host.len());
    for label in host.to_lowercase().split('.') {
        if label.is_empty() || !label.chars().all(|c| c.is_alphanumeric() || c == '-') {
            return None;
        }
        if !ascii.is_empty() {
            ascii.push('.');
        }
        if label.is_ascii() {
            ascii.push_str(label);
        } else {
            ascii.push_str("xn--");
            ascii.push_str(&punycode(label)?);
        }
    }
    Some(ascii)
}

/// No host name is longer than this, and no time is spent on one that is.
const MAX_HOST_BYTES: usize = 1024;

/// A label in another script as the ASCII it is looked up by (RFC 3492),
/// without the `xn--` in front. `None` if the arithmetic would overflow,
/// which no real label makes it do.
fn punycode(label: &str) -> Option<String> {
    const BASE: u32 = 36;
    const T_MIN: u32 = 1;
    const T_MAX: u32 = 26;
    let digit = |value: u32| char::from_digit(value, BASE);
    let adapt = |delta: u32, count: u32, first: bool| {
        let mut delta = if first { delta / 700 } else { delta / 2 };
        delta += delta / count;
        let mut k = 0;
        while delta > ((BASE - T_MIN) * T_MAX) / 2 {
            delta /= BASE - T_MIN;
            k += BASE;
        }
        k + (BASE * delta) / (delta + 38)
    };

    let input: Vec<u32> = label.chars().map(u32::from).collect();
    let mut output: String = label.chars().filter(char::is_ascii).collect();
    let basic = u32::try_from(output.len()).ok()?;
    if basic > 0 {
        output.push('-');
    }
    let total = u32::try_from(input.len()).ok()?;
    let (mut n, mut delta, mut bias, mut handled) = (128u32, 0u32, 72u32, basic);
    while handled < total {
        let next = input.iter().copied().filter(|c| *c >= n).min()?;
        delta = delta.checked_add((next - n).checked_mul(handled + 1)?)?;
        n = next;
        for &c in &input {
            if c < n {
                delta = delta.checked_add(1)?;
            }
            if c == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = (k.saturating_sub(bias)).clamp(T_MIN, T_MAX);
                    if q < t {
                        break;
                    }
                    // `char::from_digit` counts 0-9 then a-z; punycode
                    // counts a-z then 0-9.
                    output.push(digit((t + (q - t) % (BASE - t) + 10) % BASE)?);
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                output.push(digit((q + 10) % BASE)?);
                bias = adapt(delta, handled + 1, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta = delta.checked_add(1)?;
        n = n.checked_add(1)?;
    }
    Some(output)
}

/// Endings that say "file" when a word has nothing else of an address about
/// it. None of them is a top-level domain.
const FILE_ENDINGS: &[&str] = &[
    "asp", "aspx", "bmp", "css", "csv", "doc", "docx", "eml", "exe", "gif", "htm", "html", "ics",
    "jpeg", "jpg", "js", "json", "jsp", "pdf", "php", "png", "ppt", "pptx", "rtf", "txt", "xls",
    "xlsx", "xml",
];

/// Whether `shown` and `real` are the same place as far as a reader is
/// concerned: the same host, give or take `www.`, or `real` a part of the
/// site `shown` names. The other way round is not the same: a link shown as
/// `login.mybank.example` has no business going to `mybank.example`'s
/// parent, or to anywhere a longer name was needed to hide.
fn same_site(shown: &str, real: &str) -> bool {
    let shown = shown.strip_prefix("www.").unwrap_or(shown);
    let real = real.strip_prefix("www.").unwrap_or(real);
    real == shown
        || real
            .strip_suffix(shown)
            .is_some_and(|front| front.ends_with('.'))
}

/// Whether `text` could be a host name: at least two labels of letters,
/// digits and hyphens, the last one a word. Letters of any script count, so
/// that a name spelt with a lookalike letter is still seen as a name.
fn is_host_name(text: &str) -> bool {
    let text = text.strip_suffix('.').unwrap_or(text);
    let mut labels = 0;
    for label in text.split('.') {
        if label.is_empty() || !label.chars().all(|c| c.is_alphanumeric() || c == '-') {
            return false;
        }
        labels += 1;
    }
    let last = text.rsplit('.').next().unwrap_or_default();
    labels >= 2
        && (last.starts_with("xn--")
            || (last.chars().count() >= 2 && last.chars().all(char::is_alphabetic)))
}

fn is_ip_address(text: &str) -> bool {
    let mut parts = 0;
    for part in text.split('.') {
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
        parts += 1;
    }
    parts == 4
}

fn is_mail_address(text: &str) -> bool {
    match text.rsplit_once('@') {
        Some((user, host)) => {
            !user.is_empty()
                && !user.contains(['/', '@', ':'])
                && (is_host_name(host) || is_ip_address(host))
        }
        None => false,
    }
}

/// The part of a mail address after the `@`.
fn domain(address: &str) -> &str {
    address.rsplit('@').next().unwrap_or_default()
}

/// `authority` without a port number on the end.
fn without_port(authority: &str) -> &str {
    match authority.rsplit_once(':') {
        Some((host, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => host,
        _ => authority,
    }
}

/// A host as it is compared and shown: without the dot some are written
/// with at the end.
fn tidy(host: &str) -> String {
    host.strip_suffix('.').unwrap_or(host).to_owned()
}

fn shorten(text: &str) -> String {
    if text.chars().count() <= MAX_SHOWN {
        return text.to_owned();
    }
    let mut short: String = text.chars().take(MAX_SHOWN - 1).collect();
    short.push('…');
    short
}

/// Brackets, quotes and sentence punctuation around a word.
fn strip_punctuation(word: &str) -> &str {
    word.trim_start_matches(['(', '[', '{', '<', '"', '\'', '“', '‘', '«'])
        .trim_end_matches([
            ')', ']', '}', '>', '"', '\'', '”', '’', '»', '.', ',', ';', ':', '!', '?',
        ])
}

/// Characters that show nothing and could be used to break a name up.
fn is_hidden(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}' | '\u{034f}' | '\u{200b}'..='\u{200f}' | '\u{2060}'..='\u{2064}' | '\u{feff}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flagged(text: &str, href: &str) -> Option<(String, Option<String>)> {
        find(text, href).map(|found| (found.real, found.shown))
    }

    fn goes(real: &str, shown: &str) -> Option<(String, Option<String>)> {
        Some((real.to_owned(), Some(shown.to_owned())))
    }

    #[test]
    fn text_that_names_another_host_is_flagged() {
        for (text, href, real, shown) in [
            (
                "www.mybank.example",
                "https://evil.example/login",
                "evil.example",
                "www.mybank.example",
            ),
            (
                "https://mybank.example/login",
                "http://203.0.113.9/login",
                "203.0.113.9",
                "mybank.example",
            ),
            (
                "Sign in at mybank.example/login today.",
                "https://mybank.example.evil.example/login",
                "mybank.example.evil.example",
                "mybank.example",
            ),
            (
                "(MYBANK.EXAMPLE)",
                "https://evil.example",
                "evil.example",
                "mybank.example",
            ),
            (
                "http://mybank.example:8080/x",
                "https://evil.example:8080/x",
                "evil.example",
                "mybank.example",
            ),
            (
                "mybank.example",
                "https://mybank.example@evil.example/",
                "evil.example",
                "mybank.example",
            ),
            (
                "192.0.2.7",
                "https://198.51.100.7/",
                "198.51.100.7",
                "192.0.2.7",
            ),
        ] {
            assert_eq!(
                flagged(text, href),
                goes(real, shown),
                "{text:?} -> {href:?}"
            );
        }
    }

    #[test]
    fn the_same_place_is_not_flagged() {
        for (text, href) in [
            ("www.example.com", "https://www.example.com/a?b=c"),
            ("https://example.com/login", "https://example.com/login"),
            ("example.com", "https://www.example.com/"),
            ("www.example.com", "https://example.com/"),
            ("EXAMPLE.COM", "https://Example.com/"),
            ("example.com.", "https://example.com/"),
            ("http://example.com:8080/", "http://example.com:8080/x"),
            // A part of the site the text names.
            ("example.com", "https://mail.example.com/inbox"),
            ("example.com/sale", "https://shop.eu.example.com/sale?id=1"),
            ("See example.com for more", "https://example.com/more"),
            ("ana@example.com", "mailto:ana@example.com"),
            ("Ana@Example.com", "mailto:ana@example.com?subject=Hi"),
            ("help@example.com", "https://example.com/contact"),
            ("example.com", "mailto:info@example.com"),
        ] {
            assert_eq!(flagged(text, href), None, "{text:?} -> {href:?}");
        }
    }

    #[test]
    fn text_that_is_not_an_address_is_not_flagged() {
        for text in [
            "click here",
            "Sign in",
            "View the report.pdf",
            "index.html",
            "Version 2.0 is out",
            "e.g. this, i.e. that",
            "Mr. Vale et al.",
            "3.5mm",
            "U.S.A.",
            "",
            "...",
            "@handle",
            "a/b",
        ] {
            assert_eq!(flagged(text, "https://evil.example/"), None, "{text:?}");
        }
    }

    #[test]
    fn the_wrong_direction_and_near_misses_are_flagged() {
        // A longer name shown than the place gone to.
        assert_eq!(
            flagged("login.mybank.example", "https://mybank.example/"),
            goes("mybank.example", "login.mybank.example")
        );
        // The name is only the end of another word.
        assert_eq!(
            flagged("mybank.example", "https://notmybank.example/"),
            goes("notmybank.example", "mybank.example")
        );
        // Another top-level domain.
        assert_eq!(
            flagged("mybank.example", "https://mybank.example.test/"),
            goes("mybank.example.test", "mybank.example")
        );
        // A hyphen where a dot was.
        assert_eq!(
            flagged("secure.mybank.example", "https://secure-mybank.example/"),
            goes("secure-mybank.example", "secure.mybank.example")
        );
    }

    #[test]
    fn lookalike_letters_are_flagged() {
        // A Cyrillic "а" in the text, and the ASCII it stands for.
        assert_eq!(
            flagged("myb\u{430}nk.example", "https://mybank.example/"),
            goes("mybank.example", "myb\u{430}nk.example")
        );
        assert_eq!(
            flagged("mybank.example", "https://xn--mybnk-6ve.example/"),
            goes("xn--mybnk-6ve.example", "mybank.example")
        );
        // The same lookalike in both the text and the target. They agree
        // with each other; what the reader needs is the name it really is.
        assert_eq!(
            flagged("myb\u{430}nk.example", "https://myb\u{430}nk.example/login"),
            goes("xn--mybnk-6ve.example", "myb\u{430}nk.example")
        );
        assert_eq!(
            flagged(
                "www.MYB\u{410}NK.example",
                "https://www.myb\u{430}nk.example/"
            ),
            goes("www.xn--mybnk-6ve.example", "www.myb\u{430}nk.example")
        );
        // Zero-width characters hidden inside the name change nothing.
        assert_eq!(
            flagged(
                "www.my\u{200b}bank\u{2060}.exa\u{feff}mple",
                "https://evil.example/"
            ),
            goes("evil.example", "www.mybank.example")
        );
    }

    #[test]
    fn names_in_other_scripts_are_put_in_the_form_they_are_looked_up_by() {
        for (label, ascii) in [
            ("b\u{fc}cher", "bcher-kva"),
            ("myb\u{430}nk", "mybnk-6ve"),
            ("\u{65e5}\u{672c}\u{8a9e}", "wgv71a119e"),
            ("\u{fc}", "tda"),
        ] {
            assert_eq!(punycode(label).as_deref(), Some(ascii), "{label:?}");
        }
        assert_eq!(
            ascii_host("B\u{dc}CHER.Example.").as_deref(),
            Some("xn--bcher-kva.example")
        );
        assert_eq!(
            ascii_host("www.example.com").as_deref(),
            Some("www.example.com")
        );
        for not_a_host in ["", ".", "a..b", "a b.example", "evil%2eexample", "[::1]"] {
            assert_eq!(ascii_host(not_a_host), None, "{not_a_host:?}");
        }
        assert_eq!(ascii_host(&"\u{430}".repeat(2000)), None);
    }

    #[test]
    fn a_user_name_dressed_as_a_host_is_flagged_whatever_it_claims() {
        assert_eq!(
            flagged(
                "https://mybank.example@evil.example/",
                "https://mybank.example@evil.example/"
            ),
            Some(("evil.example".to_owned(), None))
        );
        assert_eq!(
            flagged("mybank.example@evil.example/login", "https://evil.example/"),
            Some(("evil.example".to_owned(), None))
        );
        assert_eq!(
            flagged(
                "https://mybank.example\\@evil.example/",
                "https://evil.example/"
            ),
            Some(("evil.example".to_owned(), None))
        );
    }

    #[test]
    fn a_target_with_no_certain_host_is_shown_whole() {
        assert_eq!(
            flagged(
                "www.mybank.example",
                "https://mybank.example\\@evil.example/"
            ),
            goes(
                "https://mybank.example\\@evil.example/",
                "www.mybank.example"
            )
        );
        assert_eq!(
            flagged("www.mybank.example", "file:///Applications/Calculator.app"),
            goes("file:///Applications/Calculator.app", "www.mybank.example")
        );
        assert_eq!(
            flagged("www.mybank.example", "https://evil%2eexample/"),
            goes("https://evil%2eexample/", "www.mybank.example")
        );
        assert_eq!(
            flagged("www.mybank.example", "/relative/path"),
            goes("/relative/path", "www.mybank.example")
        );
    }

    #[test]
    fn mail_addresses_are_compared_whole() {
        assert_eq!(
            flagged("ana@example.com", "mailto:eve@example.com"),
            goes("eve@example.com", "ana@example.com")
        );
        assert_eq!(
            flagged("ana@example.com", "mailto:ana@example.org"),
            goes("ana@example.org", "ana@example.com")
        );
        assert_eq!(
            flagged("support@mybank.example", "https://evil.example/"),
            goes("evil.example", "mybank.example")
        );
        assert_eq!(
            flagged("mybank.example", "mailto:support@evil.example"),
            goes("support@evil.example", "mybank.example")
        );
    }

    #[test]
    fn what_is_shown_is_kept_short() {
        let long = format!("{}.example", "a".repeat(200));
        let found = find("www.mybank.example", &format!("https://{long}/")).expect("flagged");
        assert_eq!(found.real.chars().count(), MAX_SHOWN);
        assert!(found.real.ends_with('…'));
        assert_eq!(
            found.warning(3),
            format!("! [3] goes to {}, not www.mybank.example", found.real)
        );
        let unclear = Disguise {
            real: "evil.example".to_owned(),
            shown: None,
        };
        assert_eq!(unclear.warning(1), "! [1] goes to evil.example");
    }

    #[test]
    fn nothing_makes_it_panic() {
        for text in [
            "",
            ".",
            "@",
            "://",
            "a://",
            "mailto:",
            "x@",
            "@x.y",
            "::::",
            "a.b@c.d@e.fg",
        ] {
            for href in [
                "",
                ":",
                "http://",
                "https://@",
                "mailto:",
                "mailto:@",
                "https://[::1]:80/",
            ] {
                let _ = find(text, href);
            }
        }
    }
}
