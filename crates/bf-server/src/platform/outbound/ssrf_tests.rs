//! SSRF guard: which addresses and URLs an outbound fetch may reach.

use super::*;
use std::net::{Ipv4Addr, Ipv6Addr};

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

#[test]
fn ipv4_ranges_that_reach_our_own_network_are_forbidden() {
    for bad in [
        "127.0.0.1", "127.255.255.254", "10.0.0.1", "10.255.255.255", "172.16.0.1", "172.31.255.255",
        "192.168.0.1", "169.254.169.254", "169.254.0.1", "0.0.0.0", "255.255.255.255", "100.64.0.1",
        "100.127.255.255", "198.18.0.1", "198.19.255.255", "192.0.0.8", "192.0.2.1", "198.51.100.1",
        "203.0.113.1", "224.0.0.1", "239.255.255.250", "240.0.0.1",
    ] {
        assert!(forbidden_ip(ip(bad)), "{bad} must be refused");
    }
}

#[test]
fn ipv4_range_edges_stay_public() {
    for ok in ["172.15.255.255", "172.32.0.1", "100.63.255.255", "100.128.0.1", "198.17.255.255", "198.20.0.1", "8.8.8.8", "1.1.1.1"] {
        assert!(!forbidden_ip(ip(ok)), "{ok} is public");
    }
}

#[test]
fn ipv6_ranges_that_reach_our_own_network_are_forbidden() {
    for bad in ["::1", "::", "fc00::1", "fdff::1", "fe80::1", "febf::1", "ff02::1", "2001:db8::1"] {
        assert!(forbidden_ip(ip(bad)), "{bad} must be refused");
    }
    assert!(!forbidden_ip(ip("2606:4700:4700::1111")));
    assert!(!forbidden_ip(ip("2a00:1450:4001::1")));
}

#[test]
fn ipv6_forms_that_carry_an_ipv4_are_judged_by_it() {
    // IPv4-mapped, any spelling.
    for bad in ["::ffff:127.0.0.1", "::ffff:7f00:1", "::ffff:169.254.169.254", "::ffff:192.168.1.1", "0:0:0:0:0:ffff:a00:1"] {
        assert!(forbidden_ip(ip(bad)), "{bad}");
    }
    assert!(!forbidden_ip(ip("::ffff:8.8.8.8")));
    // IPv4-compatible (deprecated), NAT64 and 6to4.
    for bad in ["::127.0.0.1", "::10.1.2.3", "::169.254.169.254", "64:ff9b::a00:1", "64:ff9b::a9fe:a9fe", "2002:a00:1::", "2002:c0a8:101::", "2002:a9fe:a9fe::"] {
        assert!(forbidden_ip(ip(bad)), "{bad}");
    }
    assert!(!forbidden_ip(ip("2002:808:808::")));
    assert!(!forbidden_ip(ip("::8.8.8.8")));
}

#[test]
fn every_mapped_private_address_is_refused_like_its_ipv4_form() {
    // A sweep: the mapped form must never be more permissive than the plain one.
    for a in [0u8, 1, 10, 100, 127, 169, 172, 192, 198, 203, 224, 240, 255] {
        for b in [0u8, 1, 18, 64, 127, 168, 254, 255] {
            let v4 = Ipv4Addr::new(a, b, 0, 1);
            let mapped = IpAddr::V6(v4.to_ipv6_mapped());
            let compat = IpAddr::V6(Ipv6Addr::from(u128::from(u32::from(v4))));
            let want = forbidden_ip(IpAddr::V4(v4));
            assert_eq!(forbidden_ip(mapped), want, "mapped {v4}");
            if want && u32::from(v4) > 1 {
                assert!(forbidden_ip(compat), "compat {v4}");
            }
        }
    }
}

#[test]
fn urls_with_credentials_odd_schemes_or_no_host_are_refused() {
    for bad in [
        "file:///etc/passwd", "ftp://example.com/x", "gopher://example.com/", "javascript:alert(1)", "data:text/plain,hi",
        "https://user@example.com/", "https://user:pw@example.com/", "https://:pw@example.com/",
        "https://", "not a url", "", "//example.com/x",
    ] {
        assert!(valid_url(bad).is_none(), "{bad:?} must be refused");
    }
    assert!(valid_url("https://example.com/a?b=c#d").is_some());
    assert!(valid_url("https://example.com:8443/").is_some());
}

#[test]
fn plain_http_is_refused_unless_loopback_is_allowed() {
    if loopback_allowed() {
        assert!(valid_url("http://127.0.0.1:3000/").is_some());
    } else {
        assert!(valid_url("http://example.com/").is_none());
        assert!(valid_url("http://127.0.0.1/").is_none());
        assert!(!federation_url_ok("http://example.com/inbox"));
    }
    assert!(federation_url_ok("https://example.com/inbox"));
}

#[test]
fn hosts_spelled_as_numbers_parse_to_addresses_the_guard_refuses() {
    // The url crate normalises decimal, octal and hex hosts to dotted form; the resolved
    // address is what the delivery code checks, so these must end up forbidden.
    for raw in ["https://2130706433/", "https://0x7f.0.0.1/", "https://0177.0.0.1/", "https://127.1/", "https://[::ffff:127.0.0.1]/", "https://[::1]/"] {
        let u = valid_url(raw).unwrap_or_else(|| panic!("{raw} parses"));
        let host = u.host().unwrap();
        let addr = match host {
            url::Host::Ipv4(a) => IpAddr::V4(a),
            url::Host::Ipv6(a) => IpAddr::V6(a),
            url::Host::Domain(d) => panic!("{raw} stayed a domain: {d}"),
        };
        assert!(forbidden_ip(addr), "{raw} -> {addr}");
    }
}

#[test]
fn a_name_that_resolves_to_a_mix_picks_only_a_public_address() {
    // DNS-rebinding style answer: the delivery code takes the first allowed address, so
    // filtering must skip every internal one and never return them.
    let answers: Vec<IpAddr> = ["127.0.0.1", "10.0.0.5", "::ffff:169.254.169.254", "93.184.216.34"].iter().map(|s| ip(s)).collect();
    let picked = answers.iter().find(|a| !forbidden_ip(**a));
    assert_eq!(picked, Some(&ip("93.184.216.34")));
    let all_internal: Vec<IpAddr> = ["127.0.0.1", "::1", "192.168.0.1"].iter().map(|s| ip(s)).collect();
    assert!(all_internal.iter().all(|a| forbidden_ip(*a)));
}
