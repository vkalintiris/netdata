pub(crate) mod request;
pub use request::HistogramRequest;

pub(crate) mod response;
pub use response::HistogramResponse;

mod service;
pub use service::HistogramService;
