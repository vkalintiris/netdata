//! The `nd_log` vocabulary: sources, priorities, output methods and formats, syslog facilities and the field table
//! (`src/libnetdata/log/nd_log-common.h`, `nd_log-internals.c`).

/// `ND_LOG_SOURCES`: where a record is routed; the names are the `source=` / `ND_LOG_SOURCE=` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Source {
    Unset = 0,
    Access = 1,
    Aclk = 2,
    Collector = 3,
    Daemon = 4,
    Health = 5,
    Debug = 6,
}

pub(crate) const SOURCES: usize = 7;

impl Source {
    pub(crate) const ALL: [Source; SOURCES] = [
        Source::Unset,
        Source::Access,
        Source::Aclk,
        Source::Collector,
        Source::Daemon,
        Source::Health,
        Source::Debug,
    ];

    /// `nd_log_id2source()`.
    pub fn name(self) -> &'static str {
        match self {
            Source::Unset => "UNSET",
            Source::Access => "access",
            Source::Aclk => "aclk",
            Source::Collector => "collector",
            Source::Daemon => "daemon",
            Source::Health => "health",
            Source::Debug => "debug",
        }
    }

    /// `nd_log_source2id()`: an exact name, else `default`.
    pub fn parse(name: &str, default: Source) -> Source {
        Source::ALL
            .into_iter()
            .find(|s| s.name() == name)
            .unwrap_or(default)
    }

    /// `nd_log_validate_source()` for a numeric id: out of range is the daemon source.
    pub(crate) fn from_id(id: u64) -> Source {
        Source::ALL
            .get(id as usize)
            .copied()
            .unwrap_or(Source::Daemon)
    }
}

/// `ND_LOG_FIELD_PRIORITY`: the syslog severities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Priority {
    Emerg = 0,
    Alert = 1,
    Crit = 2,
    Err = 3,
    Warning = 4,
    Notice = 5,
    Info = 6,
    Debug = 7,
}

/// `nd_log_priorities[]`: printing uses the first name of a value, parsing accepts all of them.
const PRIORITY_NAMES: [(Priority, &str); 12] = [
    (Priority::Emerg, "emergency"),
    (Priority::Emerg, "emerg"),
    (Priority::Alert, "alert"),
    (Priority::Crit, "critical"),
    (Priority::Crit, "crit"),
    (Priority::Err, "error"),
    (Priority::Err, "err"),
    (Priority::Warning, "warning"),
    (Priority::Warning, "warn"),
    (Priority::Notice, "notice"),
    (Priority::Info, "info"),
    (Priority::Debug, "debug"),
];

impl Priority {
    /// `nd_log_priority2id()`: exact and case-sensitive; anything else is info.
    pub fn parse(name: &str) -> Priority {
        PRIORITY_NAMES
            .iter()
            .find(|(_, n)| *n == name)
            .map_or(Priority::Info, |(p, _)| *p)
    }

    /// `nd_log_id2priority()`.
    pub fn name(self) -> &'static str {
        PRIORITY_NAMES
            .iter()
            .find(|(p, _)| *p == self)
            .map_or("info", |(_, n)| n)
    }

    /// `nd_log_id2priority()` for a numeric value: unknown numbers print as info.
    pub(crate) fn name_of(value: u64) -> &'static str {
        PRIORITY_NAMES
            .iter()
            .find(|(p, _)| *p as u64 == value)
            .map_or("info", |(_, n)| n)
    }
}

/// `ND_LOG_METHOD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Method {
    Disabled,
    DevNull,
    Default,
    Journal,
    Syslog,
    Stdout,
    Stderr,
    File,
}

impl Method {
    /// `nd_log_id2method()`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Method::Disabled => "none",
            Method::DevNull => "/dev/null",
            Method::Default => "default",
            Method::Journal => "journal",
            Method::Syslog => "syslog",
            Method::Stdout => "stdout",
            Method::Stderr => "stderr",
            Method::File => "file",
        }
    }

    /// `IS_VALID_LOG_METHOD_FOR_EXTERNAL_PLUGINS()` on Linux.
    pub(crate) fn valid_for_external_plugins(self) -> bool {
        matches!(self, Method::Journal | Method::Syslog | Method::Stderr)
    }
}

/// `ND_LOG_FORMAT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    Journal,
    Logfmt,
    Json,
}

impl Format {
    /// `nd_log_id2format()`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Format::Journal => "journal",
            Format::Logfmt => "logfmt",
            Format::Json => "json",
        }
    }
}

/// `nd_log_facilities[]` on Linux, in table order (`security` is `auth`).
const FACILITIES: [(&str, i32); 21] = [
    ("auth", 4 << 3),
    ("authpriv", 10 << 3),
    ("cron", 9 << 3),
    ("daemon", 3 << 3),
    ("ftp", 11 << 3),
    ("kern", 0),
    ("lpr", 6 << 3),
    ("mail", 2 << 3),
    ("news", 7 << 3),
    ("syslog", 5 << 3),
    ("user", 1 << 3),
    ("uucp", 8 << 3),
    ("local0", 16 << 3),
    ("local1", 17 << 3),
    ("local2", 18 << 3),
    ("local3", 19 << 3),
    ("local4", 20 << 3),
    ("local5", 21 << 3),
    ("local6", 22 << 3),
    ("local7", 23 << 3),
    ("security", 4 << 3),
];

/// `LOG_DAEMON`.
pub(crate) const FACILITY_DAEMON: i32 = 3 << 3;

/// `nd_log_facility2id()`: an exact name, else daemon.
pub(crate) fn facility_parse(name: &str) -> i32 {
    FACILITIES
        .iter()
        .find(|(n, _)| *n == name)
        .map_or(FACILITY_DAEMON, |(_, f)| *f)
}

/// `nd_log_id2facility()`.
pub(crate) fn facility_name(facility: i32) -> &'static str {
    FACILITIES
        .iter()
        .find(|(_, f)| *f == facility)
        .map_or("daemon", |(n, _)| n)
}

/// `ND_LOG_FIELD_ID`: a record's fields, written in this numeric order by every format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Field {
    TimestampRealtimeUsec = 1,
    SyslogIdentifier = 2,
    LogSource = 3,
    Priority = 4,
    Errno = 5,
    Winerror = 6,
    InvocationId = 7,
    Line = 8,
    File = 9,
    Func = 10,
    Tid = 11,
    ThreadTag = 12,
    MessageId = 13,
    Module = 14,
    NidlNode = 15,
    NidlInstance = 16,
    NidlContext = 17,
    NidlDimension = 18,
    SrcTransport = 19,
    AccountId = 20,
    UserName = 21,
    UserRole = 22,
    UserAccess = 23,
    SrcIp = 24,
    SrcPort = 25,
    SrcForwardedHost = 26,
    SrcForwardedFor = 27,
    SrcCapabilities = 28,
    DstTransport = 29,
    DstIp = 30,
    DstPort = 31,
    DstCapabilities = 32,
    RequestMethod = 33,
    ResponseCode = 34,
    ConnectionId = 35,
    TransactionId = 36,
    ResponseSentBytes = 37,
    ResponseSizeBytes = 38,
    ResponsePreparationTimeUsec = 39,
    ResponseSentTimeUsec = 40,
    ResponseTotalTimeUsec = 41,
    AlertId = 42,
    AlertUniqueId = 43,
    AlertEventId = 44,
    AlertTransitionId = 45,
    AlertConfigHash = 46,
    AlertName = 47,
    AlertClass = 48,
    AlertComponent = 49,
    AlertType = 50,
    AlertExec = 51,
    AlertRecipient = 52,
    AlertDuration = 53,
    AlertValue = 54,
    AlertValueOld = 55,
    AlertStatus = 56,
    AlertStatusOld = 57,
    AlertSource = 58,
    AlertUnits = 59,
    AlertSummary = 60,
    AlertInfo = 61,
    AlertNotificationRealtimeUsec = 62,
    Request = 63,
    Message = 64,
    StackTrace = 65,
}

/// `_NDF_MAX`: field slots per record, id 0 (`NDF_STOP`) included.
pub(crate) const FIELDS: usize = 66;

/// The logfmt value transformations (`logfmt_annotator`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Annotator {
    None,
    /// `timestamp_usec_annotator`: local RFC 3339 with milliseconds; 0 omits the field.
    Timestamp,
    /// `priority_annotator`: the priority name.
    Priority,
    /// `errno_annotator`: `"<n>, <strerror>"`; 0 omits the field.
    Errno,
}

/// One row of `thread_log_fields[]`: the journal and logfmt/json keys (`None`: not written in that format).
#[derive(Debug, Clone, Copy)]
pub(crate) struct FieldInfo {
    pub(crate) journal: Option<&'static str>,
    pub(crate) logfmt: Option<&'static str>,
    pub(crate) annotator: Annotator,
}

/// `thread_log_fields[]` on Linux, indexed by field id. C has two known defects kept for parity (D27): the old
/// status key is `alert_value_old`, and `NDF_ALERT_SOURCE` (58) has no keys, so it is never written.
#[rustfmt::skip]
pub(crate) const FIELD_TABLE: [FieldInfo; FIELDS] = [
    FieldInfo { journal: None, logfmt: None, annotator: Annotator::None }, // 0 NDF_STOP
    FieldInfo { journal: None, logfmt: Some("time"), annotator: Annotator::Timestamp }, // 1 NDF_TIMESTAMP_REALTIME_USEC
    FieldInfo { journal: Some("SYSLOG_IDENTIFIER"), logfmt: Some("comm"), annotator: Annotator::None }, // 2 NDF_SYSLOG_IDENTIFIER
    FieldInfo { journal: Some("ND_LOG_SOURCE"), logfmt: Some("source"), annotator: Annotator::None }, // 3 NDF_LOG_SOURCE
    FieldInfo { journal: Some("PRIORITY"), logfmt: Some("level"), annotator: Annotator::Priority }, // 4 NDF_PRIORITY
    FieldInfo { journal: Some("ERRNO"), logfmt: Some("errno"), annotator: Annotator::Errno }, // 5 NDF_ERRNO
    FieldInfo { journal: None, logfmt: None, annotator: Annotator::None }, // 6 NDF_WINERROR
    FieldInfo { journal: Some("INVOCATION_ID"), logfmt: None, annotator: Annotator::None }, // 7 NDF_INVOCATION_ID
    FieldInfo { journal: Some("CODE_LINE"), logfmt: None, annotator: Annotator::None }, // 8 NDF_LINE
    FieldInfo { journal: Some("CODE_FILE"), logfmt: None, annotator: Annotator::None }, // 9 NDF_FILE
    FieldInfo { journal: Some("CODE_FUNC"), logfmt: None, annotator: Annotator::None }, // 10 NDF_FUNC
    FieldInfo { journal: Some("TID"), logfmt: Some("tid"), annotator: Annotator::None }, // 11 NDF_TID
    FieldInfo { journal: Some("THREAD_TAG"), logfmt: Some("thread"), annotator: Annotator::None }, // 12 NDF_THREAD_TAG
    FieldInfo { journal: Some("MESSAGE_ID"), logfmt: Some("msg_id"), annotator: Annotator::None }, // 13 NDF_MESSAGE_ID
    FieldInfo { journal: Some("ND_MODULE"), logfmt: Some("module"), annotator: Annotator::None }, // 14 NDF_MODULE
    FieldInfo { journal: Some("ND_NIDL_NODE"), logfmt: Some("node"), annotator: Annotator::None }, // 15 NDF_NIDL_NODE
    FieldInfo { journal: Some("ND_NIDL_INSTANCE"), logfmt: Some("instance"), annotator: Annotator::None }, // 16 NDF_NIDL_INSTANCE
    FieldInfo { journal: Some("ND_NIDL_CONTEXT"), logfmt: Some("context"), annotator: Annotator::None }, // 17 NDF_NIDL_CONTEXT
    FieldInfo { journal: Some("ND_NIDL_DIMENSION"), logfmt: Some("dimension"), annotator: Annotator::None }, // 18 NDF_NIDL_DIMENSION
    FieldInfo { journal: Some("ND_SRC_TRANSPORT"), logfmt: Some("src_transport"), annotator: Annotator::None }, // 19 NDF_SRC_TRANSPORT
    FieldInfo { journal: Some("ND_ACCOUNT_ID"), logfmt: Some("account"), annotator: Annotator::None }, // 20 NDF_ACCOUNT_ID
    FieldInfo { journal: Some("ND_USER_NAME"), logfmt: Some("user"), annotator: Annotator::None }, // 21 NDF_USER_NAME
    FieldInfo { journal: Some("ND_USER_ROLE"), logfmt: Some("role"), annotator: Annotator::None }, // 22 NDF_USER_ROLE
    FieldInfo { journal: Some("ND_USER_PERMISSIONS"), logfmt: Some("permissions"), annotator: Annotator::None }, // 23 NDF_USER_ACCESS
    FieldInfo { journal: Some("ND_SRC_IP"), logfmt: Some("src_ip"), annotator: Annotator::None }, // 24 NDF_SRC_IP
    FieldInfo { journal: Some("ND_SRC_PORT"), logfmt: Some("src_port"), annotator: Annotator::None }, // 25 NDF_SRC_PORT
    FieldInfo { journal: Some("ND_SRC_FORWARDED_HOST"), logfmt: Some("src_forwarded_host"), annotator: Annotator::None }, // 26 NDF_SRC_FORWARDED_HOST
    FieldInfo { journal: Some("ND_SRC_FORWARDED_FOR"), logfmt: Some("src_forwarded_for"), annotator: Annotator::None }, // 27 NDF_SRC_FORWARDED_FOR
    FieldInfo { journal: Some("ND_SRC_CAPABILITIES"), logfmt: Some("src_capabilities"), annotator: Annotator::None }, // 28 NDF_SRC_CAPABILITIES
    FieldInfo { journal: Some("ND_DST_TRANSPORT"), logfmt: Some("dst_transport"), annotator: Annotator::None }, // 29 NDF_DST_TRANSPORT
    FieldInfo { journal: Some("ND_DST_IP"), logfmt: Some("dst_ip"), annotator: Annotator::None }, // 30 NDF_DST_IP
    FieldInfo { journal: Some("ND_DST_PORT"), logfmt: Some("dst_port"), annotator: Annotator::None }, // 31 NDF_DST_PORT
    FieldInfo { journal: Some("ND_DST_CAPABILITIES"), logfmt: Some("dst_capabilities"), annotator: Annotator::None }, // 32 NDF_DST_CAPABILITIES
    FieldInfo { journal: Some("ND_REQUEST_METHOD"), logfmt: Some("req_method"), annotator: Annotator::None }, // 33 NDF_REQUEST_METHOD
    FieldInfo { journal: Some("ND_RESPONSE_CODE"), logfmt: Some("code"), annotator: Annotator::None }, // 34 NDF_RESPONSE_CODE
    FieldInfo { journal: Some("ND_CONNECTION_ID"), logfmt: Some("conn"), annotator: Annotator::None }, // 35 NDF_CONNECTION_ID
    FieldInfo { journal: Some("ND_TRANSACTION_ID"), logfmt: Some("transaction"), annotator: Annotator::None }, // 36 NDF_TRANSACTION_ID
    FieldInfo { journal: Some("ND_RESPONSE_SENT_BYTES"), logfmt: Some("sent_bytes"), annotator: Annotator::None }, // 37 NDF_RESPONSE_SENT_BYTES
    FieldInfo { journal: Some("ND_RESPONSE_SIZE_BYTES"), logfmt: Some("size_bytes"), annotator: Annotator::None }, // 38 NDF_RESPONSE_SIZE_BYTES
    FieldInfo { journal: Some("ND_RESPONSE_PREP_TIME_USEC"), logfmt: Some("prep_ut"), annotator: Annotator::None }, // 39 NDF_RESPONSE_PREPARATION_TIME_USEC
    FieldInfo { journal: Some("ND_RESPONSE_SENT_TIME_USEC"), logfmt: Some("sent_ut"), annotator: Annotator::None }, // 40 NDF_RESPONSE_SENT_TIME_USEC
    FieldInfo { journal: Some("ND_RESPONSE_TOTAL_TIME_USEC"), logfmt: Some("total_ut"), annotator: Annotator::None }, // 41 NDF_RESPONSE_TOTAL_TIME_USEC
    FieldInfo { journal: Some("ND_ALERT_ID"), logfmt: Some("alert_id"), annotator: Annotator::None }, // 42 NDF_ALERT_ID
    FieldInfo { journal: Some("ND_ALERT_UNIQUE_ID"), logfmt: Some("alert_unique_id"), annotator: Annotator::None }, // 43 NDF_ALERT_UNIQUE_ID
    FieldInfo { journal: Some("ND_ALERT_EVENT_ID"), logfmt: Some("alert_event_id"), annotator: Annotator::None }, // 44 NDF_ALERT_EVENT_ID
    FieldInfo { journal: Some("ND_ALERT_TRANSITION_ID"), logfmt: Some("alert_transition_id"), annotator: Annotator::None }, // 45 NDF_ALERT_TRANSITION_ID
    FieldInfo { journal: Some("ND_ALERT_CONFIG"), logfmt: Some("alert_config"), annotator: Annotator::None }, // 46 NDF_ALERT_CONFIG_HASH
    FieldInfo { journal: Some("ND_ALERT_NAME"), logfmt: Some("alert"), annotator: Annotator::None }, // 47 NDF_ALERT_NAME
    FieldInfo { journal: Some("ND_ALERT_CLASS"), logfmt: Some("alert_class"), annotator: Annotator::None }, // 48 NDF_ALERT_CLASS
    FieldInfo { journal: Some("ND_ALERT_COMPONENT"), logfmt: Some("alert_component"), annotator: Annotator::None }, // 49 NDF_ALERT_COMPONENT
    FieldInfo { journal: Some("ND_ALERT_TYPE"), logfmt: Some("alert_type"), annotator: Annotator::None }, // 50 NDF_ALERT_TYPE
    FieldInfo { journal: Some("ND_ALERT_EXEC"), logfmt: Some("alert_exec"), annotator: Annotator::None }, // 51 NDF_ALERT_EXEC
    FieldInfo { journal: Some("ND_ALERT_RECIPIENT"), logfmt: Some("alert_recipient"), annotator: Annotator::None }, // 52 NDF_ALERT_RECIPIENT
    FieldInfo { journal: Some("ND_ALERT_DURATION"), logfmt: Some("alert_duration"), annotator: Annotator::None }, // 53 NDF_ALERT_DURATION
    FieldInfo { journal: Some("ND_ALERT_VALUE"), logfmt: Some("alert_value"), annotator: Annotator::None }, // 54 NDF_ALERT_VALUE
    FieldInfo { journal: Some("ND_ALERT_VALUE_OLD"), logfmt: Some("alert_value_old"), annotator: Annotator::None }, // 55 NDF_ALERT_VALUE_OLD
    FieldInfo { journal: Some("ND_ALERT_STATUS"), logfmt: Some("alert_status"), annotator: Annotator::None }, // 56 NDF_ALERT_STATUS
    FieldInfo { journal: Some("ND_ALERT_STATUS_OLD"), logfmt: Some("alert_value_old"), annotator: Annotator::None }, // 57 NDF_ALERT_STATUS_OLD
    FieldInfo { journal: None, logfmt: None, annotator: Annotator::None }, // 58 NDF_ALERT_SOURCE
    FieldInfo { journal: Some("ND_ALERT_UNITS"), logfmt: Some("alert_units"), annotator: Annotator::None }, // 59 NDF_ALERT_UNITS
    FieldInfo { journal: Some("ND_ALERT_SUMMARY"), logfmt: Some("alert_summary"), annotator: Annotator::None }, // 60 NDF_ALERT_SUMMARY
    FieldInfo { journal: Some("ND_ALERT_INFO"), logfmt: Some("alert_info"), annotator: Annotator::None }, // 61 NDF_ALERT_INFO
    FieldInfo { journal: Some("ND_ALERT_NOTIFICATION_TIMESTAMP_USEC"), logfmt: Some("alert_notification_timestamp"), annotator: Annotator::Timestamp }, // 62 NDF_ALERT_NOTIFICATION_REALTIME_USEC
    FieldInfo { journal: Some("ND_REQUEST"), logfmt: Some("request"), annotator: Annotator::None }, // 63 NDF_REQUEST
    FieldInfo { journal: Some("MESSAGE"), logfmt: Some("msg"), annotator: Annotator::None }, // 64 NDF_MESSAGE
    FieldInfo { journal: Some("ND_STACK_TRACE"), logfmt: None, annotator: Annotator::None }, // 65 NDF_STACK_TRACE
];

/// Well-known `MESSAGE_ID`s (`src/libnetdata/uuid/uuid.h`), as bytes.
pub mod msgid {
    const fn hex(s: &[u8; 32]) -> [u8; 16] {
        const fn nibble(c: u8) -> u8 {
            match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                _ => panic!("not a lowercase hex digit"),
            }
        }
        let mut out = [0u8; 16];
        let mut i = 0;
        while i < 16 {
            out[i] = nibble(s[2 * i]) << 4 | nibble(s[2 * i + 1]);
            i += 1;
        }
        out
    }

    pub const STARTUP: [u8; 16] = hex(b"1e6061a9fbd44501b3ccc368119f2b69");
    pub const EXIT: [u8; 16] = hex(b"02f47d350af5449197bf7a95b605a468");
    pub const FATAL: [u8; 16] = hex(b"23e93dfccbf64e11aac858b9410d8a82");
    pub const HEALTH_ALERT_TRANSITION: [u8; 16] = hex(b"9ce0cb58ab8b44df82c4bf1ad9ee22de");
    pub const STREAMING_FROM_CHILD: [u8; 16] = hex(b"ed4cdb8f1beb4ad3b57cb3cae2d162fa");
    pub const STREAMING_TO_PARENT: [u8; 16] = hex(b"6e2e3839067648968b646045dbf28d66");
    pub const ACLK_CONNECTION: [u8; 16] = hex(b"acb33cb95778476baac702eb7e4e151d");
    pub const LOG_FLOOD_PROTECTION: [u8; 16] = hex(b"ec87a56120d5431bace51e2fb8bba243");
    pub const EXTREME_CARDINALITY: [u8; 16] = hex(b"d1f59606dd4d41e3b217a0cfcae8e632");
    pub const DYNCFG_USER_ACTION: [u8; 16] = hex(b"4fdf40816c124623a032b7fe73beacb8");
}
