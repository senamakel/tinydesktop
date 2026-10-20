//! Tests for canonical site normalization shared by the host and module.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use super::{site_named, site_of};

#[test]
fn site_urls_use_url_idna_and_reject_non_http_and_ip_hosts() {
    assert_eq!(
        site_of(" https://www.bücher.example/shop ").as_deref(),
        Some("xn--bcher-kva.example")
    );
    assert_eq!(
        site_of("http://shop.test:8080/cart").as_deref(),
        Some("shop.test")
    );
    for url in [
        "ftp://shop.test/",
        "file:///tmp/page.html",
        "not a URL",
        "https://[::1]/",
    ] {
        assert_eq!(site_of(url), None, "{url}");
    }
}

#[test]
fn site_names_are_ascii_normalized_and_safe_file_stems() {
    assert_eq!(
        site_named("  WWW.Example.COM  ").as_deref(),
        Some("example.com")
    );
    assert_eq!(site_named("example.com.").as_deref(), Some("example.com."));
    assert_eq!(site_named("bücher.example"), None);
    assert_eq!(site_named(".hidden"), None);
    assert_eq!(site_named("a..b"), None);
    assert_eq!(site_named(&"a".repeat(254)), None);
    let longest = "a".repeat(253);
    assert_eq!(site_named(&longest).as_deref(), Some(longest.as_str()));
}
