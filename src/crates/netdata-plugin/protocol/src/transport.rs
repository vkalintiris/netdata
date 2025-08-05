use crate::message_parser::{Message, MessageParser};
use futures::{SinkExt, StreamExt};
use std::fmt;
use tokio::io::{stdin, stdout, AsyncRead, AsyncWrite, Stdin, Stdout};
use tokio_util::codec::{FramedRead, FramedWrite};

/// Error type for ProtocolTransport operations
#[derive(Debug)]
pub enum TransportError {
    /// IO error during transport operations
    Io(std::io::Error),
    /// Protocol parsing error - contains the debug representation of the error
    Protocol(String),
    /// Transport is closed
    Closed,
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::Io(e) => write!(f, "IO error: {}", e),
            TransportError::Protocol(e) => write!(f, "Protocol error: {}", e),
            TransportError::Closed => write!(f, "Transport is closed"),
        }
    }
}

impl std::error::Error for TransportError {}

impl From<std::io::Error> for TransportError {
    fn from(e: std::io::Error) -> Self {
        TransportError::Io(e)
    }
}

/// Transport for bidirectional communication over Netdata's external plugin protocol
///
/// This transport handles the framed reading and writing of protocol messages
/// over any AsyncRead/AsyncWrite streams, abstracting away the codec details.
pub struct Transport<R, W>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    reader: FramedRead<R, MessageParser>,
    writer: FramedWrite<W, MessageParser>,
}

impl<R, W> Transport<R, W>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    /// Create a new ProtocolTransport with custom reader and writer streams
    pub fn new_with_streams(reader: R, writer: W) -> Self {
        let reader = FramedRead::new(reader, MessageParser::input());
        let writer = FramedWrite::new(writer, MessageParser::output());

        Self { reader, writer }
    }

    /// Send a message through the transport
    ///
    /// This method handles the send and flush operations automatically
    pub async fn send(&mut self, message: Message) -> Result<(), TransportError> {
        self.writer.send(message).await?;
        self.writer.flush().await?;
        Ok(())
    }

    /// Receive the next message from the transport
    ///
    /// Returns None if the transport is closed, or an error if parsing fails
    pub async fn recv(&mut self) -> Option<Result<Message, TransportError>> {
        match self.reader.next().await {
            Some(Ok(message)) => Some(Ok(message)),
            Some(Err(e)) => Some(Err(TransportError::Protocol(format!("{:?}", e)))),
            None => None,
        }
    }

    /// Send a message and receive the next response
    ///
    /// This is a convenience method for request-response patterns
    pub async fn request(&mut self, message: Message) -> Result<Option<Message>, TransportError> {
        self.send(message).await?;

        match self.recv().await {
            Some(Ok(response)) => Ok(Some(response)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }
}

impl Transport<Stdin, Stdout> {
    /// Create a new ProtocolTransport using stdin/stdout
    pub fn new() -> Self {
        Self::new_with_streams(stdin(), stdout())
    }
}

impl Default for Transport<Stdin, Stdout> {
    fn default() -> Self {
        Self::new()
    }
}
