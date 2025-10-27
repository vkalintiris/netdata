use allocative::Allocative;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct Response {
    pub available_histograms: Vec<available_histogram::AvailableHistogram>,
    pub histogram: histogram::Histogram,
}

pub mod available_histogram {
    use super::*;

    #[derive(Debug, Serialize, Deserialize)]
    #[cfg_attr(feature = "allocative", derive(Allocative))]
    pub struct AvailableHistogram {
        pub id: String,
        pub name: String,
        pub order: usize,
    }
}

pub mod histogram {
    use super::*;

    #[derive(Debug, Serialize, Deserialize)]
    #[cfg_attr(feature = "allocative", derive(Allocative))]
    pub struct Histogram {
        pub id: String,
        pub name: String,
        pub chart: chart::Chart,
    }

    pub mod chart {
        use super::*;

        #[derive(Debug, Serialize, Deserialize)]
        #[cfg_attr(feature = "allocative", derive(Allocative))]
        pub struct Chart {
            pub view: view::View,
            pub result: result::Result,
        }

        pub mod view {
            use super::*;

            #[derive(Debug, Serialize, Deserialize)]
            #[cfg_attr(feature = "allocative", derive(Allocative))]
            pub struct View {
                pub title: String,
                pub after: u32,
                pub before: u32,
                pub units: String,
                pub chart_type: String,
                pub dimensions: Dimensions,
            }

            #[derive(Debug, Serialize, Deserialize)]
            #[cfg_attr(feature = "allocative", derive(Allocative))]
            pub struct Dimensions {
                pub ids: Vec<String>,
                pub names: Vec<String>,
                pub units: Vec<String>,
            }
        }

        pub mod result {
            use super::*;
            use serde::Serializer;

            #[derive(Debug, Serialize, Deserialize)]
            #[cfg_attr(feature = "allocative", derive(Allocative))]
            pub struct Result {
                pub labels: Vec<String>,
                pub point: Point,
                pub data: Vec<DataItem>,
            }

            #[derive(Debug, Serialize, Deserialize)]
            #[cfg_attr(feature = "allocative", derive(Allocative))]
            pub struct Point {
                pub value: u64,
                pub arp: u64,
                pub pa: u64,
            }

            #[derive(Debug, Deserialize)]
            #[serde(from = "(u64, Vec<[usize; 3]>)")]
            #[cfg_attr(feature = "allocative", derive(Allocative))]
            pub struct DataItem {
                pub timestamp: u64,
                pub items: Vec<[usize; 3]>,
            }

            impl Serialize for DataItem {
                fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
                where
                    S: Serializer,
                {
                    (&self.timestamp, &self.items).serialize(serializer)
                }
            }

            impl From<(u64, Vec<[usize; 3]>)> for DataItem {
                fn from((timestamp, items): (u64, Vec<[usize; 3]>)) -> Self {
                    Self { timestamp, items }
                }
            }
        }
    }
}
