#![allow(unused_imports, dead_code)]

use tokio::runtime::{Builder, Handle, Runtime};
use tokio::sync::mpsc;

use hello_world::greeter_client::GreeterClient;
use hello_world::HelloRequest;

use crate::Error;
type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub struct Message {
    pub msg: String,
}

#[derive(Debug)]
pub struct Client {
    rt_handle: Handle,
    tx_msg: mpsc::Sender<Message>,
}

impl Client {
    pub fn new() -> Result<Client> {
        let (tx_msg, mut rx_msg) = mpsc::channel(16);

        let rt = match Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(err) => return Err(Error::runtime(err)),
        };
        let rt_handle = rt.handle().clone();

        std::thread::spawn(move || {
            rt.block_on(async move {
                while let Some(msg) = rx_msg.recv().await {
                    tokio::spawn(say_hello(msg));
                }

                // Once all senders have gone out of scope,
                // the `.recv()` call returns None and it will
                // exit from the while loop and shut down the
                // thread.
            });
        });

        Ok(Client { rt_handle, tx_msg })
    }

    pub fn spawn_task(&self, msg: Message) {
        match self.tx_msg.blocking_send(msg) {
            Ok(()) => {}
            Err(_) => panic!("The shared runtime has shut down."),
        }
    }
}

pub mod hello_world {
    tonic::include_proto!("helloworld");
}

pub async fn say_hello(m: Message) {
    let mut client = GreeterClient::connect("http://[::1]:50051").await.unwrap();

    let request = tonic::Request::new(HelloRequest { name: m.msg });

    let response = client.say_hello(request).await.unwrap();

    println!("RESPONSE={:?}", response);
}
