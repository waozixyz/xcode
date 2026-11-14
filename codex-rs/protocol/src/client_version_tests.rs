use super::TRACKED_UPSTREAM_CLIENT_VERSION;
use super::is_newer;
use super::normalize;
use super::record_latest_upstream_version;
use super::reported_client_version;
use pretty_assertions::assert_eq;

#[test]
fn release_tags_normalize_and_prereleases_are_rejected() {
    assert_eq!(normalize("rust-v0.170.2").as_deref(), Some("0.170.2"));
    assert_eq!(normalize("v1.2.3").as_deref(), Some("1.2.3"));
    assert_eq!(normalize(" 0.171.0\n").as_deref(), Some("0.171.0"));
    assert_eq!(normalize("0.171.0-alpha.1"), None);
    assert_eq!(normalize("0.171"), None);
    assert_eq!(normalize("0.171.0.1"), None);
}

#[test]
fn versions_compare_numerically() {
    assert!(is_newer("0.170.0", "0.169.9"));
    assert!(is_newer("1.0.0", "0.999.0"));
    assert!(!is_newer("0.169.0", "0.169.0"));
    assert!(!is_newer("garbage", "0.1.0"));
}

// One test owns the process-wide latest version, so the assertions run in order.
#[test]
fn reported_version_follows_the_newest_upstream_release_and_never_goes_back() {
    assert_eq!(reported_client_version(), TRACKED_UPSTREAM_CLIENT_VERSION);
    assert_eq!(record_latest_upstream_version("0.1.0"), None);
    assert_eq!(record_latest_upstream_version("not a version"), None);
    assert_eq!(reported_client_version(), TRACKED_UPSTREAM_CLIENT_VERSION);

    assert_eq!(
        record_latest_upstream_version("rust-v9.8.7").as_deref(),
        Some("9.8.7")
    );
    assert_eq!(reported_client_version(), "9.8.7");
    assert_eq!(record_latest_upstream_version("9.8.6"), None);
    assert_eq!(reported_client_version(), "9.8.7");
}
