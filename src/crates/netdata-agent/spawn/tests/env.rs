//! The frozen environment (D134.4): a variable set after the snapshot reaches the children's block, in its place,
//! and never the process's own environment. One test: the snapshot is the process's.

use netdata_agent_spawn::env;

#[test]
fn a_frozen_environment_changes_in_its_snapshot() {
    env::freeze();
    let before = env::block();
    assert!(before.iter().any(|e| e.as_bytes().starts_with(b"PATH=")));
    env::set("ND_SPAWN_TEST", "1").unwrap();
    env::set("ND_SPAWN_TEST", "2").unwrap();
    let after = env::block();
    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(after.last().map(|e| e.as_bytes().to_vec()), Some(b"ND_SPAWN_TEST=2".to_vec()));
    assert_eq!(env::get("ND_SPAWN_TEST"), Some("2".into()));
    assert_eq!(std::env::var_os("ND_SPAWN_TEST"), None);
}
