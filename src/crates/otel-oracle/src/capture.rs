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

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

/// Reads a capture one record at a time, never past `limit` bytes of it (the
/// header included): the runner passes the length it saw while the tee was
/// frozen, so records written after that are left alone. A record that runs
/// past the limit or the end of the input is one the tee was still writing:
/// reading stops before it and [`Reader::cut`] says how much of it was there.
pub struct Reader<R> {
    input: R,
    limit: Option<u64>,
    offset: u64,
    cut: Option<u64>,
}

impl<R: Read> Reader<R> {
    pub fn new(input: R, limit: Option<u64>) -> io::Result<Reader<R>> {
        let mut reader = Reader {
            input,
            limit,
            offset: 0,
            cut: None,
        };
        let mut magic = Vec::new();
        reader.read_up_to(MAGIC.len() as u64, &mut magic)?;
        if magic != MAGIC {
            return Err(invalid("not an otel-tee capture"));
        }
        Ok(reader)
    }

    /// Bytes read so far, the header included.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// The bytes seen of the record reading stopped before, when it was cut
    /// short.
    pub fn cut(&self) -> Option<u64> {
        self.cut
    }

    /// Appends up to `want` bytes to `buf`, fewer at the limit or the end of
    /// the input, and returns how many. Nothing is allocated ahead of the
    /// bytes, so a corrupt length costs only what is there.
    fn read_up_to(&mut self, want: u64, buf: &mut Vec<u8>) -> io::Result<u64> {
        let mut want = want;
        if let Some(limit) = self.limit {
            want = want.min(limit.saturating_sub(self.offset));
        }
        let read = (&mut self.input).take(want).read_to_end(buf)? as u64;
        self.offset += read;
        Ok(read)
    }

    /// The next record, or `None` at the end and before a record cut short.
    pub fn next_record(&mut self) -> io::Result<Option<Record>> {
        if self.cut.is_some() {
            return Ok(None);
        }
        let mut len = Vec::new();
        let got = self.read_up_to(4, &mut len)?;
        if got == 0 {
            return Ok(None);
        }
        let Ok(len) = <[u8; 4]>::try_from(len.as_slice()) else {
            self.cut = Some(got);
            return Ok(None);
        };
        let len = u64::from(u32::from_le_bytes(len));
        if len < FIXED as u64 {
            return Err(invalid("a record is shorter than its fixed fields"));
        }
        let mut record = Vec::new();
        let got = self.read_up_to(len, &mut record)?;
        if got < len {
            self.cut = Some(4 + got);
            return Ok(None);
        }
        parse(record).map(Some)
    }
}

/// A record after its length field, `FIXED` bytes or more.
fn parse(mut record: Vec<u8>) -> io::Result<Record> {
    let i64_at = |at: usize| {
        let mut le = [0; 8];
        le.copy_from_slice(&record[at..at + 8]);
        i64::from_le_bytes(le)
    };
    let received_unix_ns = i64_at(0);
    let rejected = i64_at(13);
    let signal = Signal::of(record[8]).ok_or_else(|| invalid("unknown signal"))?;
    let mut code = [0; 4];
    code.copy_from_slice(&record[9..13]);
    let request = record.split_off(FIXED);
    Ok(Record {
        received_unix_ns,
        signal,
        grpc_code: i32::from_le_bytes(code),
        rejected,
        request,
    })
}

/// Every record of a capture. A record cut short (the tee stopped while
/// writing it) is an error, not silently dropped.
pub fn read_all(input: &mut impl Read) -> io::Result<Vec<Record>> {
    let mut reader = Reader::new(input, None)?;
    let mut records = Vec::new();
    while let Some(record) = reader.next_record()? {
        records.push(record);
    }
    if reader.cut().is_some() {
        return Err(invalid("a record is cut short"));
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

    fn record(at: i64, request: Vec<u8>) -> Record {
        Record {
            received_unix_ns: at,
            signal: Signal::Traces,
            grpc_code: 0,
            rejected: 0,
            request,
        }
    }

    /// The capture's bytes and where each record ends.
    fn capture(records: &[Record]) -> (Vec<u8>, Vec<u64>) {
        let mut out = Vec::new();
        write_header(&mut out).unwrap();
        let mut ends = Vec::new();
        for record in records {
            write_record(&mut out, record).unwrap();
            ends.push(out.len() as u64);
        }
        (out, ends)
    }

    #[test]
    fn a_reader_stops_at_its_limit_and_before_a_record_cut_short() {
        let (bytes, ends) = capture(&[
            record(1, vec![1; 10]),
            record(2, vec![2; 20]),
            record(3, vec![3; 30]),
        ]);
        let read = |input: &[u8], limit: Option<u64>| {
            let mut reader = Reader::new(input, limit).unwrap();
            let mut got = Vec::new();
            while let Some(record) = reader.next_record().unwrap() {
                got.push(record.received_unix_ns);
            }
            assert_eq!(reader.next_record().unwrap(), None);
            (got, reader.cut(), reader.offset())
        };
        let last = ends[2] as usize;
        let cases = [
            ("no limit", &bytes[..], None, (vec![1, 2, 3], None, ends[2])),
            (
                "a limit on a record's end",
                &bytes[..],
                Some(ends[1]),
                (vec![1, 2], None, ends[1]),
            ),
            (
                "a limit inside a record",
                &bytes[..],
                Some(ends[1] + 7),
                (vec![1, 2], Some(7), ends[1] + 7),
            ),
            (
                "a limit inside a length",
                &bytes[..],
                Some(ends[0] + 3),
                (vec![1], Some(3), ends[0] + 3),
            ),
            (
                "an input cut inside a record",
                &bytes[..last - 1],
                None,
                (vec![1, 2], Some(ends[2] - ends[1] - 1), ends[2] - 1),
            ),
            (
                "a limit past the end",
                &bytes[..],
                Some(ends[2] + 100),
                (vec![1, 2, 3], None, ends[2]),
            ),
        ];
        for (name, input, limit, expected) in cases {
            assert_eq!(read(input, limit), expected, "{name}");
        }
    }

    #[test]
    fn a_cut_record_stays_cut_while_the_tee_finishes_writing_it() {
        let (bytes, ends) = capture(&[record(1, vec![1; 10]), record(2, vec![2; 20])]);
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let half = (ends[0] + 9) as usize;
        file.write_all(&bytes[..half]).unwrap();
        let input = std::fs::File::open(file.path()).unwrap();
        let mut reader = Reader::new(input, None).unwrap();

        assert!(reader.next_record().unwrap().is_some());
        assert_eq!(reader.next_record().unwrap(), None);
        file.write_all(&bytes[half..]).unwrap();

        assert_eq!(reader.next_record().unwrap(), None);
        assert_eq!(reader.cut(), Some(9));
    }

    #[test]
    fn a_corrupt_record_is_an_error_not_a_cut() {
        let (bytes, _) = capture(&[record(1, vec![1; 4])]);
        let first = MAGIC.len();

        let mut unknown = bytes.clone();
        unknown[first + 4 + 8] = 9;
        let mut short = bytes.clone();
        short[first..first + 4].copy_from_slice(&(FIXED as u32 - 1).to_le_bytes());
        for (name, input) in [("an unknown signal", unknown), ("a short length", short)] {
            let mut reader = Reader::new(input.as_slice(), None).unwrap();
            assert!(reader.next_record().is_err(), "{name}");
        }
        assert!(Reader::new(&bytes[..first - 1], None).is_err());
        assert!(Reader::new(bytes.as_slice(), Some(first as u64 - 1)).is_err());
    }
}
