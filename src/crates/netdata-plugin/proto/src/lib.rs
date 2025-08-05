#![cfg_attr(docsrs, feature(doc_auto_cfg))]
//! This crate contains generated protobuf files for Netdata's external plugin protocol
//! and provides type-safe Rust structures for protocol messages.
//!
//! # Feature flags
//!
//! ## Code generation
//! - `gen-tonic-messages`: Generate message types using [tonic](https://github.com/hyperium/tonic) and [prost](https://github.com/tokio-rs/prost)
//! - `gen-tonic`: Generate gRPC client/server code using [tonic](https://github.com/hyperium/tonic) (includes `gen-tonic-messages`)
//!
//! ## Message types
//! - `functions`: Include function-related protocol messages
//!
//! ## Service types
//! - `agent`: Include Netdata agent service definitions
//! - `plugin`: Include Netdata plugin service definitions
//!
//! ## Serialization
//! - `with-serde`: Add serde serialization support to generated types
//!
//! ## Misc
//! - `full`: Enable all features above
//!
//! By default, the `full` feature is enabled.

// Tonic generated code - skip formatting and lint checks
#[rustfmt::skip]
#[allow(warnings)]
#[doc(hidden)]
#[cfg(feature = "gen-tonic-messages")]
pub mod tonic {
    pub mod netdata {
        pub mod protocol {
            pub mod v1 {
                #[cfg(feature = "functions")]
                include!("proto/tonic/netdata.protocol.v1.rs");
                
                #[cfg(feature = "agent")]
                pub mod agent {
                    include!("proto/tonic/netdata.protocol.v1.agent.rs");
                }

                #[cfg(feature = "plugin")]
                pub mod plugin {
                    include!("proto/tonic/netdata.protocol.v1.plugin.rs");
                }
            }
        }
    }
}

// Re-export the generated types for easier access
#[cfg(feature = "gen-tonic-messages")]
pub use crate::tonic::netdata::protocol::v1;
