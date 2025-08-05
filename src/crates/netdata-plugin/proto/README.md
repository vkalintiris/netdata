# Protocol Proto

This crate contains generated protobuf files for Netdata's external plugin protocol and provides type-safe Rust structures for protocol messages.

## Features

This crate uses a feature-flag based approach similar to `opentelemetry-proto` to provide modular functionality:

### Code Generation
- **`gen-prost`**: Generate message types using [prost](https://github.com/tokio-rs/prost)
- **`gen-tonic`**: Generate gRPC client/server code using [tonic](https://github.com/hyperium/tonic) (includes `gen-prost`)

### Message Types  
- **`functions`**: Include function-related protocol messages

### Service Types
- **`agent`**: Include Netdata agent service definitions for gRPC

### Serialization
- **`with-serde`**: Add serde serialization support to generated types

### Convenience
- **`full`**: Enable all features above (default)

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
protocol-proto = { path = "../protocol-proto" }

# Or with specific features
protocol-proto = { path = "../protocol-proto", features = ["gen-prost", "functions"] }

# Without serde if you don't need JSON serialization  
protocol-proto = { path = "../protocol-proto", features = ["gen-prost", "functions"], default-features = false }
```

## Examples

This crate includes several examples:

- `basic_usage` - Basic protobuf message creation and serialization
- `simple_integration` - Simple integration with the protocol parsing
- `netdata_protocol_integration` - Comprehensive example showing text-to-protobuf conversion

Run examples with:
```bash
cargo run --example basic_usage
cargo run --example simple_integration
cargo run --example netdata_protocol_integration
```

## Basic Example

```rust
use protocol_proto::proto::FunctionDeclaration;

fn main() {
    let func_decl = FunctionDeclaration {
        name: "get_system_info".to_string(),
        timeout: 30,
        help: "Returns system information".to_string(),
        source: "system_plugin".to_string(),
        access_flags: 0,
        priority: 100,
    };
    
    println!("Function: {}", func_decl.name);
}
```

## With Serde Support

```rust
use protocol_proto::proto::FunctionDeclaration;

fn main() {
    let func_decl = FunctionDeclaration {
        name: "get_system_info".to_string(),
        timeout: 30,
        help: "Returns system information".to_string(), 
        source: "system_plugin".to_string(),
        access_flags: 0,
        priority: 100,
    };
    
    // Serialize to JSON
    let json = serde_json::to_string(&func_decl).unwrap();
    println!("JSON: {}", json);
    
    // Deserialize from JSON
    let parsed: FunctionDeclaration = serde_json::from_str(&json).unwrap();
    println!("Parsed: {}", parsed.name);
}
```

## gRPC Service Usage

With the `gen-tonic` feature, you can implement gRPC services:

```rust
use protocol_proto::proto::FunctionDeclaration;
use protocol_proto::v1::agent::netdata_service_server::{NetdataService, NetdataServiceServer};
use protocol_proto::v1::agent::{DeclareFunctionRequest, DeclareFunctionResponse};

#[derive(Debug, Default)]
pub struct MyNetdataService;

#[tonic::async_trait]
impl NetdataService for MyNetdataService {
    async fn declare_function(
        &self,
        request: tonic::Request<DeclareFunctionRequest>,
    ) -> Result<tonic::Response<DeclareFunctionResponse>, tonic::Status> {
        let req = request.into_inner();
        
        if let Some(declaration) = req.declaration {
            println!("Received function declaration: {}", declaration.name);
            
            let response = DeclareFunctionResponse {
                success: true,
                error_message: None,
            };
            
            Ok(tonic::Response::new(response))
        } else {
            Err(tonic::Status::invalid_argument("Missing function declaration"))
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "0.0.0.0:50051".parse()?;
    let service = MyNetdataService::default();

    tonic::transport::Server::builder()
        .add_service(NetdataServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
```

## Project Structure

```
protocol-proto/
├── proto/                                    # Protobuf definitions
│   └── netdata/
│       └── protocol/
│           └── v1/
│               ├── functions.proto           # Function-related messages
│               └── agent/
│                   └── netdata_service.proto # gRPC service definition
├── src/
│   ├── lib.rs                               # Library entry point and re-exports
│   ├── generated/                           # Checked-in prost generated files
│   │   ├── netdata.protocol.v1.rs           # Generated from functions.proto
│   │   └── netdata.protocol.v1.agent.rs     # Generated from netdata_service.proto
│   └── proto/
│       └── tonic/                           # Checked-in tonic generated files
│           ├── netdata.protocol.v1.rs       # Messages with gRPC support
│           └── netdata.protocol.v1.agent.rs # Service client/server code
├── tests/
│   └── proto_build.rs                       # Tests for code generation validation
└── examples/
    ├── basic_usage.rs                       # Basic message usage
    └── basic_grpc_usage.rs                  # gRPC service usage
```

## Generated Files Workflow

This crate follows the **checked-in generated files** approach (similar to `opentelemetry-proto`):

- Generated Rust files are **committed to the repository** in `src/generated/`
- No build-time code generation - faster builds and no build dependencies
- Tests validate that checked-in files match what would be generated

## Adding New Message Types

1. Create a new `.proto` file in the appropriate `proto/netdata/protocol/v*/` directory
2. Regenerate the Rust files:
   ```bash
   cargo test ensure_generated_files_are_up_to_date
   ```
   If this test fails, it will automatically update the generated files
3. Add feature flags in `Cargo.toml` if needed  
4. Update `src/lib.rs` to include and re-export the new generated types
5. Add tests in `tests/proto_build.rs`
6. **Commit the updated generated files** to the repository

## Updating Existing Messages

When you modify a `.proto` file:

1. Run the validation test:
   ```bash
   cargo test ensure_generated_files_are_up_to_date
   ```
2. The test will fail and automatically update `src/generated/*.rs` files
3. Review and commit the changes to the generated files

## Transport Abstraction

The crate is designed to support different protobuf/gRPC implementations:

- **`gen-prost`**: Uses `prost` for protobuf message generation (lightweight, message types only)  
- **`gen-tonic`**: Uses `tonic` + `prost` for full gRPC client/server support (when you add services)

This allows you to choose the right level of functionality for your use case - messages only, or full gRPC support.