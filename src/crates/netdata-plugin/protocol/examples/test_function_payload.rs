use netdata_plugin_proto::v1::FunctionCall;
use netdata_plugin_protocol::{Message, Transport};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("Testing function payload begin/end support...");

    // Create a loopback transport using duplex streams
    let (client_stream, server_stream) = tokio::io::duplex(8192);

    let (server_read, server_write) = tokio::io::split(server_stream);
    let mut server_transport = Transport::new_with_streams(server_read, server_write);

    let (client_read, client_write) = tokio::io::split(client_stream);
    let mut client_transport = Transport::new_with_streams(client_read, client_write);

    // Test 1: Send a FunctionCall message with payload (will encode to FUNCTION_PAYLOAD protocol format)
    let function_call = Message::FunctionCall(Box::new(FunctionCall {
        transaction: "test-tx-123".to_string(),
        timeout: 30,
        function: "test_function".to_string(),
        access: Some(0x01),
        source: Some("test-source".to_string()),
        payload: Some(b"Hello!".to_vec()),
    }));

    // Send message from client
    {
        eprintln!("Sending message: {:#?}", function_call);
        if let Err(e) = client_transport.send(function_call).await {
            eprintln!("Error sending message: {}", e);
            return Err(e.into());
        }
        eprintln!("Message sent successfully!");
    }

    // Receive message on server side
    {
        let message = server_transport.recv().await;

        match message {
            Some(Ok(message)) => eprintln!("Received message: {:#?}", message),
            Some(Err(e)) => eprintln!("Error receiving message : {}", e),
            None => eprintln!("Transport closed"),
        }
    }

    // Test 2: Send a FunctionCall message without payload (will encode to FUNCTION protocol format)
    eprintln!("\n--- Testing FunctionCall without payload ---");
    let function_call_no_payload = Message::FunctionCall(Box::new(FunctionCall {
        transaction: "test-tx-456".to_string(),
        timeout: 15,
        function: "simple_function".to_string(),
        access: None,
        source: None,
        payload: None,
    }));

    // Send message from client
    {
        eprintln!("Sending message: {:#?}", function_call_no_payload);
        if let Err(e) = client_transport.send(function_call_no_payload).await {
            eprintln!("Error sending message: {}", e);
            return Err(e.into());
        }
        eprintln!("Message sent successfully!");
    }

    // Receive message on server side
    {
        let message = server_transport.recv().await;

        match message {
            Some(Ok(message)) => eprintln!("Received message: {:#?}", message),
            Some(Err(e)) => eprintln!("Error receiving message : {}", e),
            None => eprintln!("Transport closed"),
        }
    }

    eprintln!("\nTest completed!");
    Ok(())
}
