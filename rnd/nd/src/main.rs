#![allow(unused_imports, dead_code)]

use iclient::{Client, Message};
use odb::ODB;
use std::{thread, time};

fn main() {
    let mut odb = ODB::new();
    let mut _oid = odb.add("what?");

    let c = match Client::new() {
        Ok(c) => c,
        Err(e) => panic!("err: {:?}", e),
    };

    c.spawn_task(Message {
        msg: String::from("Vasileios"),
    });

    thread::sleep(time::Duration::from_secs(1))
}
