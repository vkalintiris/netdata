// #![allow(unused_imports, dead_code)]

use iclient::say_hello;
use odb::ODB;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut odb = ODB::new();
    let mut _oid = odb.add("what?");

    say_hello().await?;
    Ok(())
}
