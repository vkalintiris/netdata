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

        println!("Received logs export request:");
        for resource_log in req.resource_logs {
            if let Some(resource) = &resource_log.resource {
                println!("Resource attributes: {:?}", resource.attributes);
            }

            for scope_log in resource_log.scope_logs {
                if let Some(scope) = &scope_log.scope {
                    println!("Scope: {}", scope.name);
                }

                for log_record in scope_log.log_records {
                    println!("Log Record:");
                    println!("  Time: {:?}", log_record.time_unix_nano);
                    println!("  Severity: {:?}", log_record.severity_text);
                    println!("  Body: {:?}", log_record.body);
                    println!("  Attributes: {:?}", log_record.attributes);
                    println!("---");
                }
            }
        }

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
