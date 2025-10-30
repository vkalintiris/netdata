#[cfg(feature = "allocative")]
use allocative::Allocative;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct Response {
    pub facets: Vec<facet::Facet>,
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

pub mod facet {
    use super::*;

    #[derive(Debug, Serialize, Deserialize)]
    #[cfg_attr(feature = "allocative", derive(Allocative))]
    pub struct Facet {
        pub id: String,
        pub name: String,
        pub order: usize,
        pub options: Vec<Option>,
    }

    #[derive(Debug, Serialize, Deserialize)]
    #[cfg_attr(feature = "allocative", derive(Allocative))]
    pub struct Option {
        pub id: String,
        pub name: String,
        pub order: usize,
        pub count: usize,
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

        fn remap_strings(vec: &mut Vec<String>, map: &HashMap<&str, &str>) {
            for s in vec.iter_mut() {
                if let Some(&new_value) = map.get(s.as_str()) {
                    *s = new_value.to_string();
                }
            }
        }

        #[derive(Debug, Serialize, Deserialize)]
        #[cfg_attr(feature = "allocative", derive(Allocative))]
        pub struct Chart {
            pub view: view::View,
            pub result: result::Result,
        }

        impl Chart {
            pub fn patch_priority(&mut self) {
                let mut map = HashMap::default();

                map.insert("0", "emergency");
                map.insert("1", "alert");
                map.insert("2", "critical");
                map.insert("3", "error");
                map.insert("4", "warning");
                map.insert("5", "notice");
                map.insert("6", "info");
                map.insert("7", "debug");

                remap_strings(&mut self.view.dimensions.ids, &map);
                remap_strings(&mut self.view.dimensions.names, &map);
                remap_strings(&mut self.result.labels, &map);
            }
        }

        pub mod view {
            use super::*;

            #[derive(Debug, Serialize, Deserialize)]
            #[cfg_attr(feature = "allocative", derive(Allocative))]
            pub struct View {
                pub title: String,
                pub after: u32,
                pub before: u32,
                pub update_every: u32,
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

            #[derive(Debug)]
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
                    use serde::ser::SerializeSeq;

                    // Create a sequence with length = 1 (timestamp) + number of items
                    let mut seq = serializer.serialize_seq(Some(1 + self.items.len()))?;

                    // First element: timestamp
                    seq.serialize_element(&self.timestamp)?;

                    // Remaining elements: each [usize; 3] array
                    for item in &self.items {
                        seq.serialize_element(item)?;
                    }

                    seq.end()
                }
            }

            impl<'de> Deserialize<'de> for DataItem {
                fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
                where
                    D: serde::Deserializer<'de>,
                {
                    use serde::de::{SeqAccess, Visitor};

                    struct DataItemVisitor;

                    impl<'de> Visitor<'de> for DataItemVisitor {
                        type Value = DataItem;

                        fn expecting(
                            &self,
                            formatter: &mut std::fmt::Formatter,
                        ) -> std::fmt::Result {
                            formatter.write_str("an array with timestamp followed by data items")
                        }

                        fn visit_seq<A>(
                            self,
                            mut seq: A,
                        ) -> std::result::Result<Self::Value, A::Error>
                        where
                            A: SeqAccess<'de>,
                        {
                            // First element: timestamp
                            let timestamp = seq
                                .next_element()?
                                .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;

                            // Remaining elements: collect all [usize; 3] arrays
                            let mut items = Vec::new();
                            while let Some(item) = seq.next_element()? {
                                items.push(item);
                            }

                            Ok(DataItem { timestamp, items })
                        }
                    }

                    deserializer.deserialize_seq(DataItemVisitor)
                }
            }
        }
    }
}
