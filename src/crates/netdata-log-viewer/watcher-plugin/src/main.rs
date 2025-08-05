use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Bind to localhost:9999
    let listener = TcpListener::bind("127.0.0.1:9999").await?;
    eprintln!("Listening on localhost:9999");

    // Accept incoming connections
    loop {
        let (mut socket, addr) = listener.accept().await?;
        eprintln!("Connection from: {}", addr);

        // Spawn a task to handle this connection
        tokio::spawn(async move {
            let (mut tcp_reader, mut tcp_writer) = socket.split();
            let mut stdin = tokio::io::stdin();
            let mut stdout = tokio::io::stdout();

            // Create two concurrent tasks for bidirectional copying
            let tcp_to_stdout = async {
                let mut buf = [0; 8192];
                loop {
                    match tcp_reader.read(&mut buf).await {
                        Ok(0) => {
                            eprintln!("Connection closed by client");
                            break;
                        }
                        Ok(n) => {
                            if let Err(e) = stdout.write_all(&buf[..n]).await {
                                eprintln!("Error writing to stdout: {}", e);
                                break;
                            }
                            if let Err(e) = stdout.flush().await {
                                eprintln!("Error flushing stdout: {}", e);
                                break;
                            }
                        }
                        Err(e) => {
                            eprintln!("Error reading from socket: {}", e);
                            break;
                        }
                    }
                }
            };

            let stdin_to_tcp = async {
                let mut buf = [0; 8192];
                loop {
                    match stdin.read(&mut buf).await {
                        Ok(0) => {
                            eprintln!("EOF on stdin");
                            break;
                        }
                        Ok(n) => {
                            if let Err(e) = tcp_writer.write_all(&buf[..n]).await {
                                eprintln!("Error writing to socket: {}", e);
                                break;
                            }
                            if let Err(e) = tcp_writer.flush().await {
                                eprintln!("Error flushing socket: {}", e);
                                break;
                            }
                        }
                        Err(e) => {
                            eprintln!("Error reading from stdin: {}", e);
                            break;
                        }
                    }
                }
            };

            // Run both tasks concurrently and wait for either to complete
            tokio::select! {
                _ = tcp_to_stdout => {
                    eprintln!("TCP to stdout completed");
                }
                _ = stdin_to_tcp => {
                    eprintln!("Stdin to TCP completed");
                }
            }

            eprintln!("Connection handler terminating");
        });
    }
}
