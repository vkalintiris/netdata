use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct AvailableHistogram {
    pub id: String,
    pub name: String,
    pub order: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Point {
    value: u64,
    arp: u64,
    pa: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChartResult {
    labels: Vec<String>,
    data: (),
    point: Point,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Chart {
    #[serde(rename(serialize = "result"))]
    pub chart_result: ChartResult,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FieldHistogram {
    pub id: String,
    pub name: String,
}
