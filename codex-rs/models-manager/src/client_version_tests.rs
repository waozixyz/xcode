use super::TRACKED_UPSTREAM_CLIENT_VERSION;
use super::client_version_to_whole;
use pretty_assertions::assert_eq;

#[test]
fn catalog_reports_the_tracked_upstream_release() {
    assert_eq!(client_version_to_whole(), TRACKED_UPSTREAM_CLIENT_VERSION);
}
