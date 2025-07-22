use netdata_bridge::{
    netdata::{PingRequest, PongResponse},
    NetdataPlugin,
};
use tonic::{Request, Response, Status};

#[derive(Default)]
pub struct MyPlugin;

#[tonic::async_trait]
impl NetdataPlugin for MyPlugin {
    async fn ping(&self, _request: Request<PingRequest>) -> Result<Response<PongResponse>, Status> {
        Ok(Response::new(PongResponse {
            message: "Hello from my plugin!".to_string(),
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let plugin = MyPlugin::default();
    netdata_bridge::run_plugin(plugin).await
}
