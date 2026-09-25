//! The capture file `otel-tee` writes: every traces and logs export request the
//! lab agent was sent, in arrival order, with when it arrived and how the agent
//! answered. The calculator reads its spans from here on live data, so they
//! never pass through the plugin's decoding.
//!
//! Layout: the 8-byte magic, then one record after another, integers little
//! endian:
//!
//! | bytes | field |
//! |---|---|
//! | 4 | length of the rest of the record |
//! | 8 | receive time, unix nanoseconds |
//! | 1 | signal: 1 traces, 2 logs |
//! | 4 | the agent's gRPC status code (0 = OK) |
//! | 8 | spans or log records the agent reported rejecting |
//! | rest | the export request, uncompressed protobuf, as the agent received it |

use std::io::{self, Read, Write};

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;

pub const MAGIC: &[u8; 8] = b"OTEECAP1";

/// Bytes of a record after its length field, before the request.
const FIXED: usize = 8 + 1 + 4 + 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Traces,
    Logs,
}

impl Signal {
    fn byte(self) -> u8 {
        match self {
            Signal::Traces => 1,
            Signal::Logs => 2,
        }
    }

    fn of(byte: u8) -> Option<Signal> {
        match byte {
            1 => Some(Signal::Traces),
            2 => Some(Signal::Logs),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub received_unix_ns: i64,
    pub signal: Signal,
    pub grpc_code: i32,
    pub rejected: i64,
    pub request: Vec<u8>,
}

impl Record {
    /// Whether the agent answered OK. An acknowledged request may still have
    /// rejected spans outside the ingestion window; the rest were stored.
    pub fn acknowledged(&self) -> bool {
        self.grpc_code == 0
    }

    /// Whether the agent stored all of it: acknowledged with nothing rejected.
    pub fn accepted(&self) -> bool {
        self.acknowledged() && self.rejected == 0
    }

    pub fn traces(&self) -> Result<ExportTraceServiceRequest, prost::DecodeError> {
        ExportTraceServiceRequest::decode(self.request.as_slice())
    }

    pub fn logs(&self) -> Result<ExportLogsServiceRequest, prost::DecodeError> {
        ExportLogsServiceRequest::decode(self.request.as_slice())
    }
}

pub fn write_header(out: &mut impl Write) -> io::Result<()> {
    out.write_all(MAGIC)
}

pub fn write_record(out: &mut impl Write, record: &Record) -> io::Result<()> {
    let len = u32::try_from(FIXED + record.request.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "record over 4 GiB"))?;
    out.write_all(&len.to_le_bytes())?;
    out.write_all(&record.received_unix_ns.to_le_bytes())?;
    out.write_all(&[record.signal.byte()])?;
    out.write_all(&record.grpc_code.to_le_bytes())?;
    out.write_all(&record.rejected.to_le_bytes())?;
    out.write_all(&record.request)
}

/// Every record of a capture. A record cut short (the tee stopped while
/// writing it) is an error, not silently dropped.
pub fn read_all(input: &mut impl Read) -> io::Result<Vec<Record>> {
    let invalid = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_string());
    let mut bytes = Vec::new();
    input.read_to_end(&mut bytes)?;
    let Some(mut rest) = bytes.strip_prefix(MAGIC.as_slice()) else {
        return Err(invalid("not an otel-tee capture"));
    };
    let mut records = Vec::new();
    while !rest.is_empty() {
        let (len, body) = rest
            .split_first_chunk::<4>()
            .ok_or_else(|| invalid("a record length is cut short"))?;
        let len = u32::from_le_bytes(*len) as usize;
        if len < FIXED || body.len() < len {
            return Err(invalid("a record is cut short"));
        }
        let (record, after) = body.split_at(len);
        let i64_at = |at: usize| {
            let mut le = [0; 8];
            le.copy_from_slice(&record[at..at + 8]);
            i64::from_le_bytes(le)
        };
        let signal = Signal::of(record[8]).ok_or_else(|| invalid("unknown signal"))?;
        let mut code = [0; 4];
        code.copy_from_slice(&record[9..13]);
        records.push(Record {
            received_unix_ns: i64_at(0),
            signal,
            grpc_code: i32::from_le_bytes(code),
            rejected: i64_at(13),
            request: record[FIXED..].to_vec(),
        });
        rest = after;
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_and_a_cut_record_is_an_error() {
        let records = [
            Record {
                received_unix_ns: 1_758_791_650_123_456_789,
                signal: Signal::Traces,
                grpc_code: 0,
                rejected: 0,
                request: vec![1, 2, 3],
            },
            Record {
                received_unix_ns: -1,
                signal: Signal::Logs,
                grpc_code: 14,
                rejected: 7,
                request: Vec::new(),
            },
        ];
        let mut out = Vec::new();
        write_header(&mut out).unwrap();
        for record in &records {
            write_record(&mut out, record).unwrap();
        }
        assert_eq!(read_all(&mut out.as_slice()).unwrap(), records);
        assert!(records[0].accepted() && !records[1].accepted());
        assert!(records[0].acknowledged() && !records[1].acknowledged());

        for cut in [out.len() - 1, MAGIC.len() + 2] {
            assert!(read_all(&mut &out[..cut]).is_err(), "cut at {cut}");
        }
        assert!(read_all(&mut &b"garbage!"[..]).is_err());
        let mut empty = Vec::new();
        write_header(&mut empty).unwrap();
        assert_eq!(read_all(&mut empty.as_slice()).unwrap(), []);
    }
}
