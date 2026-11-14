use super::CachedRelease;
use super::REFRESH_AFTER;
use super::needs_refresh;
use super::read_cache;
use super::write_cache;
use pretty_assertions::assert_eq;

#[test]
fn cache_round_trips() {
    let home = tempfile::tempdir().unwrap();
    let cached = CachedRelease {
        version: "0.170.0".to_string(),
        checked_at: 1_000,
    };
    write_cache(home.path(), &cached).unwrap();
    assert_eq!(read_cache(home.path()), Some(cached));
}

#[test]
fn missing_or_old_cache_is_refreshed() {
    let now = 10 * REFRESH_AFTER.as_secs();
    let fresh = CachedRelease {
        version: "0.170.0".to_string(),
        checked_at: now - 60,
    };
    let old = CachedRelease {
        version: "0.170.0".to_string(),
        checked_at: now - REFRESH_AFTER.as_secs(),
    };
    assert!(needs_refresh(None, now));
    assert!(!needs_refresh(Some(&fresh), now));
    assert!(needs_refresh(Some(&old), now));
}
