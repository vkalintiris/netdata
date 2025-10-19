#![allow(unused_imports)]

use journal::index_state::IndexState;
use journal::registry::Registry;
use journal::repository::File;

use std::collections::HashSet;

fn main() {
    let mut registry = Registry::new().unwrap();
    registry.watch_directory("/var/log/journal").unwrap();

    let mut indexed_fields = HashSet::new();
    indexed_fields.insert(String::from("PRIORITY"));

    let mut index_state = IndexState::new(registry, indexed_fields);

    println!("Hello there!");
}
