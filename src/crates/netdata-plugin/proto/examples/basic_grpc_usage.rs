use netdata_plugin_proto::v1::FunctionDeclaration;

#[cfg(feature = "gen-tonic")]
use netdata_plugin_proto::v1::agent::netdata_service_server::{
    NetdataService, NetdataServiceServer,
};
#[cfg(feature = "gen-tonic")]
use netdata_plugin_proto::v1::agent::DeclareFunctionResponse;

#[cfg(feature = "gen-tonic")]
#[derive(Debug, Default)]
pub struct MyNetdataService;

#[cfg(feature = "gen-tonic")]
#[tonic::async_trait]
impl NetdataService for MyNetdataService {
    async fn declare_function(
        &self,
        request: tonic::Request<FunctionDeclaration>,
    ) -> Result<tonic::Response<DeclareFunctionResponse>, tonic::Status> {
        let declaration = request.into_inner();

        println!("Received function declaration: {}", declaration.name);

        let response = DeclareFunctionResponse {
            success: true,
            error_message: None,
        };

        Ok(tonic::Response::new(response))
    }
}

fn main() {
    // Basic message usage
    let func_decl = FunctionDeclaration {
        global: false,
        name: "get_system_info".to_string(),
        timeout: 30,
        help: "Returns system information".to_string(),
        tags: Some("system,info".to_string()),
        access: Some(0),
        priority: Some(100),
        version: Some(1),
    };

    println!("Created function declaration: {}", func_decl.name);

    #[cfg(feature = "gen-tonic")]
    {
        // gRPC service usage example
        let service = MyNetdataService::default();
        let _server = NetdataServiceServer::new(service);
        println!("gRPC service created successfully!");
    }

    #[cfg(not(feature = "gen-tonic"))]
    {
        println!("gRPC features not enabled - only message types available");
    }
}
