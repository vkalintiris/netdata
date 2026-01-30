//! Output formatting for Netdata external plugin protocol.

use std::io::{self, Write};

/// Output writer for the Netdata external plugin protocol.
///
/// This is a stateless writer - it just formats and writes protocol commands.
/// State tracking (which charts/dimensions have been defined) is handled by `ChartState`.
pub struct NetdataOutput<W: Write> {
    writer: W,
}

impl NetdataOutput<io::Stdout> {
    /// Create a new NetdataOutput that writes to stdout.
    pub fn stdout() -> Self {
        Self::new(io::stdout())
    }
}

impl<W: Write> NetdataOutput<W> {
    /// Create a new NetdataOutput with a custom writer.
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    /// Write the CHART definition command.
    pub fn write_chart_definition(
        &mut self,
        id: &str,
        title: &str,
        units: &str,
        family: &str,
        update_every: u64,
    ) {
        // CHART type.id name title units family context charttype priority update_every
        // We use 'line' chart type, priority 1000, and the configured update_every
        let _ = writeln!(
            self.writer,
            "CHART {} '' '{}' '{}' '{}' '' line 1000 {}",
            id, title, units, family, update_every
        );
    }

    /// Write the DIMENSION definition command.
    pub fn write_dimension_definition(&mut self, dim_name: &str) {
        // DIMENSION id name algorithm multiplier divisor
        // We use 'absolute' algorithm since we handle aggregation ourselves
        // Divisor of 1000000 gives us 6 decimal places of precision
        let _ = writeln!(
            self.writer,
            "DIMENSION {} '{}' absolute 1 1000000",
            dim_name, dim_name
        );
    }

    /// Write the BEGIN command.
    pub fn write_begin(&mut self, chart_id: &str) {
        let _ = writeln!(self.writer, "BEGIN {}", chart_id);
    }

    /// Write the SET command for a dimension value.
    pub fn write_set(&mut self, dim_name: &str, value: f64) {
        // Netdata expects integer values. We multiply by 1000000 for precision
        // (matching the divisor in DIMENSION definition)
        let scaled = (value * 1_000_000.0).round() as i64;
        let _ = writeln!(self.writer, "SET {} = {}", dim_name, scaled);
    }

    /// Write the END command with explicit timestamp.
    pub fn write_end(&mut self, timestamp_secs: u64) {
        let _ = writeln!(self.writer, "END {}", timestamp_secs);
    }

    /// Flush the output to ensure all data is written.
    pub fn flush(&mut self) {
        let _ = self.writer.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn writes_chart_definition() {
        let buf = Cursor::new(Vec::new());
        let mut output = NetdataOutput::new(buf);

        output.write_chart_definition("system.cpu", "CPU Usage", "percentage", "cpu", 1);

        let result = String::from_utf8_lossy(output.writer.get_ref()).to_string();
        assert!(result.contains("CHART system.cpu '' 'CPU Usage' 'percentage' 'cpu' '' line 1000 1"));
    }

    #[test]
    fn writes_dimension_definition() {
        let buf = Cursor::new(Vec::new());
        let mut output = NetdataOutput::new(buf);

        output.write_dimension_definition("user");

        let result = String::from_utf8_lossy(output.writer.get_ref()).to_string();
        assert!(result.contains("DIMENSION user 'user' absolute 1 1000000"));
    }

    #[test]
    fn writes_update_sequence() {
        let buf = Cursor::new(Vec::new());
        let mut output = NetdataOutput::new(buf);

        output.write_begin("system.cpu");
        output.write_set("user", 50.5);
        output.write_set("system", 25.25);
        output.write_end(1704067200);

        let result = String::from_utf8_lossy(output.writer.get_ref()).to_string();
        assert!(result.contains("BEGIN system.cpu"));
        assert!(result.contains("SET user = 50500000")); // 50.5 * 1000000
        assert!(result.contains("SET system = 25250000")); // 25.25 * 1000000
        assert!(result.contains("END 1704067200"));
    }
}
