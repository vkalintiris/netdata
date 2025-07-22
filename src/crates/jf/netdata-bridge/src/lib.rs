use tonic::transport::{Endpoint, Server, Uri};
use tonic::Request;
use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt, BufReader};
use hyper_util::rt::TokioIo;
use tower::service_fn;

pub mod netdata {
    tonic::include_proto!("netdata");
}

pub use netdata::netdata_plugin_server::NetdataPlugin;
use netdata::{
    netdata_plugin_server::NetdataPluginServer,
    netdata_plugin_client::NetdataPluginClient,
    PingRequest,
};

pub async fn run_plugin<T>(plugin: T) -> Result<(), Box<dyn std::error::Error>>
where
    T: NetdataPlugin + Send + Sync + 'static,
{
    // Create in-memory duplex channel
    let (client_io, server_io) = duplex(1024);

    // Spawn the gRPC server
    tokio::spawn(async move {
        Server::builder()
            .add_service(NetdataPluginServer::new(plugin))
            .serve_with_incoming(tokio_stream::once(Ok::<_, std::io::Error>(server_io)))
            .await
            .unwrap();
    });

    // Create client
    let mut client_io = Some(client_io);
    let channel = Endpoint::try_from("http://dummy")?
        .connect_with_connector(service_fn(move |_: Uri| {
            let client = client_io.take();
            async move {
                if let Some(client) = client {
                    Ok(TokioIo::new(client))
                } else {
                    Err(std::io::Error::other("Client already taken"))
                }
            }
        }))
        .await?;

    let mut client = NetdataPluginClient::new(channel);

    // Main loop: read from stdin, write to stdout
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut stdout = tokio::io::stdout();
    let mut line = String::new();

    loop {
        line.clear();
        
        match reader.read_line(&mut line).await? {
            0 => break, // EOF
            _ => {
                if line.trim() == "PING" {
                    let request = Request::new(PingRequest {});
                    let response = client.ping(request).await?;
                    let pong = response.into_inner();
                    
                    let output = format!("PONG {}\n", pong.message);
                    stdout.write_all(output.as_bytes()).await?;
                    stdout.flush().await?;
                }
            }
        }
    }

    Ok(())
}