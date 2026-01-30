//! Output formatting for metrics emission.

/// A dimension value ready for output.
pub struct DimensionValue<'a> {
    pub name: &'a str,
    pub value: Option<f64>,
}

/// Trait for emitting metrics to various outputs.
pub trait MetricsOutput {
    /// Emit a chart update.
    fn emit_update(
        &mut self,
        chart_name: &str,
        timestamp: u64,
        dimensions: &[DimensionValue<'_>],
    );
}

/// Debug output that prints to stdout.
#[derive(Default)]
pub struct DebugOutput;

impl MetricsOutput for DebugOutput {
    fn emit_update(
        &mut self,
        chart_name: &str,
        timestamp: u64,
        dimensions: &[DimensionValue<'_>],
    ) {
        println!(
            "CHART {} @ {} (slot_timestamp={})",
            chart_name, timestamp, timestamp
        );

        for dim in dimensions {
            match dim.value {
                Some(v) => println!("  DIM {} = {:.6}", dim.name, v),
                None => println!("  DIM {} = U", dim.name),
            }
        }
    }
}
