use flatten_otel_logs::flatten_export_logs_request;
use opentelemetry_proto::tonic::collector::logs::v1::{
    logs_service_server::{LogsService, LogsServiceServer},
    ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use tonic::{transport::Server, Request, Response, Status};

#[derive(Debug, Default)]
pub struct MyLogsService {}

#[tonic::async_trait]
impl LogsService for MyLogsService {
    async fn export(
        &self,
        request: Request<ExportLogsServiceRequest>,
    ) -> Result<Response<ExportLogsServiceResponse>, Status> {
        let req = request.into_inner();

        let _result = flatten_export_logs_request(&req).unwrap();
        // for (idx, item) in result.iter().enumerate() {
        //     println!(">>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>");
        //     println!("Displaying flattened log: {:?}", idx);
        //     println!("{:#?}", item);
        //     println!("<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<");
        // }

        let reply = ExportLogsServiceResponse {
            partial_success: None,
        };

        Ok(Response::new(reply))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "127.0.0.1:20000".parse()?;
    let logs_service = MyLogsService::default();

    println!("Starting OTEL logs receiver on {}", addr);

    Server::builder()
        .add_service(LogsServiceServer::new(logs_service))
        .serve(addr)
        .await?;

    Ok(())
}
