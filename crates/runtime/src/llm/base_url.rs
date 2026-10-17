//! Base URLs as settings name them (LLD §3.8, §9): whether one is this Mac, and whether two
//! name the same endpoint however they are spelled (a trailing slash, the host's case, the
//! default port, `localhost` or `127.0.0.1`).

use std::net::IpAddr;

use reqwest::Url;

/// The URL's host is this machine (`localhost`, `127.0.0.0/8`, `::1`): traffic to it never
/// leaves the Mac, so a key may go to it over plain HTTP.
#[must_use]
pub fn is_loopback(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    // An IPv6 host is written in brackets (`[::1]`).
    let address = host.trim_start_matches('[').trim_end_matches(']');
    address
        .parse::<IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}

/// `a` and `b` name the same endpoint: the same scheme, host (or both this machine), port, path
/// (a trailing slash aside) and query. A URL that doesn't parse names nothing.
#[must_use]
pub fn same_endpoint(a: &str, b: &str) -> bool {
    let (Ok(a), Ok(b)) = (Url::parse(a), Url::parse(b)) else {
        return false;
    };
    let hosts = a.host_str() == b.host_str() || (is_loopback(&a) && is_loopback(&b));
    a.scheme() == b.scheme()
        && hosts
        && a.port_or_known_default() == b.port_or_known_default()
        && a.path().trim_end_matches('/') == b.path().trim_end_matches('/')
        && a.query() == b.query()
}

#[cfg(test)]
mod tests {
    use reqwest::Url;

    use super::{is_loopback, same_endpoint};

    #[test]
    fn this_mac_is_localhost_or_a_loopback_address() {
        for local in [
            "http://localhost:11434",
            "http://LocalHost/v1",
            "http://127.0.0.1:1234",
            "http://127.8.0.1",
            "http://[::1]:8080",
        ] {
            assert!(
                Url::parse(local).is_ok_and(|url| is_loopback(&url)),
                "{local}"
            );
        }
        for remote in [
            "http://openrouter.ai",
            "http://localhost.example.com",
            "http://10.0.0.2",
            "http://[2001:db8::1]",
        ] {
            assert!(
                Url::parse(remote).is_ok_and(|url| !is_loopback(&url)),
                "{remote}"
            );
        }
    }

    #[test]
    fn an_endpoint_is_the_same_however_it_is_spelled() {
        assert!(same_endpoint(
            "https://openrouter.ai/api/v1/",
            "https://OPENROUTER.ai:443/api/v1"
        ));
        assert!(same_endpoint(
            "http://127.0.0.1:11434/v1",
            "http://localhost:11434/v1/"
        ));
        assert!(!same_endpoint(
            "http://openrouter.ai/api/v1",
            "https://openrouter.ai/api/v1"
        ));
        assert!(!same_endpoint(
            "http://localhost:1234/v1",
            "http://localhost:11434/v1"
        ));
        assert!(!same_endpoint("not a url", "not a url"));
    }
}
