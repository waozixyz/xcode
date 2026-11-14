use super::TRACKED_UPSTREAM_CLIENT_VERSION;
use super::reported_client_version;
use super::reported_version;
use pretty_assertions::assert_eq;

#[test]
fn development_version_reports_tracked_upstream_release() {
    assert_eq!(reported_version("0.0.0"), TRACKED_UPSTREAM_CLIENT_VERSION);
    assert_eq!(reported_version("0.0.0-dev"), TRACKED_UPSTREAM_CLIENT_VERSION);
}

#[test]
fn released_version_is_reported_without_suffix() {
    assert_eq!(reported_version("0.170.2"), "0.170.2");
    assert_eq!(reported_version("1.2.3-alpha.4"), "1.2.3");
}

#[test]
fn service_never_sees_the_development_placeholder() {
    assert_ne!(reported_client_version(), "0.0.0");
}
