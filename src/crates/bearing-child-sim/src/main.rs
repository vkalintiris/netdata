use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

const DEFAULT_ADDR: &str = "127.0.0.1:19999";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or(DEFAULT_ADDR.into());
    let child_name = std::env::args().nth(2).unwrap_or("child-sim-1".into());

    eprintln!("connecting to {addr} as {child_name}...");

    let stream = TcpStream::connect(&addr)
        .await
        .expect("failed to connect");

    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    // Send the BEARING HTTP method to trigger connection upgrade.
    // Must include " HTTP/1.1\r\n" — the web server's parser looks for it.
    write_half
        .write_all(b"BEARING / HTTP/1.1\r\n\r\n")
        .await
        .expect("failed to send BEARING handshake");

    // Read the welcome response from the coordinator.
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .expect("failed to read welcome");
    let welcome = line.trim();
    eprintln!("received: {welcome}");

    if !welcome.starts_with("BEARING OK") {
        eprintln!("unexpected welcome response, aborting");
        return;
    }

    // Send READY.
    line.clear();
    let ready_msg = format!("READY {child_name}\n");
    write_half
        .write_all(ready_msg.as_bytes())
        .await
        .expect("failed to send READY");

    eprintln!("connected and ready");

    // Read queries and echo them back.
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                eprintln!("coordinator disconnected");
                break;
            }
            Err(e) => {
                eprintln!("read error: {e}");
                break;
            }
            Ok(_) => {}
        }

        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("QUERY ") {
            if let Some((id_str, query_text)) = rest.split_once(' ') {
                eprintln!("received query {id_str}: {query_text}");
                let response = format!("RESULT {id_str} {{\"echo\":\"{query_text}\"}}\n");
                if let Err(e) = write_half.write_all(response.as_bytes()).await {
                    eprintln!("write error: {e}");
                    break;
                }
            }
        }
    }
}
