//! The keyword table (`src/plugins.d/gperf-hashtable.h`): names, which parsers accept them, and the traffic class
//! streaming accounts them under.

/// Repertoire bits (`PARSER_INIT_*`, `PARSER_REP_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Repertoire(u8);

impl Repertoire {
    /// Accepted from external plugins.
    pub const PLUGINSD: Self = Self(1 << 0);
    /// Accepted from a streaming child.
    pub const STREAMING: Self = Self(1 << 1);
    pub const REPLICATION: Self = Self(1 << 3);
    pub const METADATA: Self = Self(1 << 4);
    pub const DATA: Self = Self(1 << 5);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

macro_rules! keywords {
    ($($variant:ident = $name:literal : $($rep:ident)|+;)+) => {
        /// A protocol keyword.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Keyword { $($variant,)+ }

        const TABLE: &[(Keyword, &str, Repertoire)] = &[
            $((Keyword::$variant, $name, Repertoire(0)$(.with(Repertoire::$rep))+),)+
        ];
    };
}

keywords! {
    Begin = "BEGIN": PLUGINSD | STREAMING | DATA;
    Begin2 = "BEGIN2": STREAMING | DATA;
    Chart = "CHART": PLUGINSD | STREAMING | METADATA | REPLICATION;
    ChartDefinitionEnd = "CHART_DEFINITION_END": STREAMING | REPLICATION | METADATA;
    ClaimedId = "CLAIMED_ID": STREAMING | METADATA;
    Clabel = "CLABEL": PLUGINSD | STREAMING | METADATA;
    ClabelCommit = "CLABEL_COMMIT": PLUGINSD | STREAMING | METADATA;
    Config = "CONFIG": PLUGINSD | METADATA;
    DeleteJob = "DELETE_JOB": PLUGINSD | STREAMING;
    Dimension = "DIMENSION": PLUGINSD | STREAMING | METADATA;
    Disable = "DISABLE": PLUGINSD;
    DyncfgEnable = "DYNCFG_ENABLE": PLUGINSD | STREAMING;
    DyncfgRegisterJob = "DYNCFG_REGISTER_JOB": PLUGINSD | STREAMING;
    DyncfgRegisterModule = "DYNCFG_REGISTER_MODULE": PLUGINSD | STREAMING;
    DyncfgReset = "DYNCFG_RESET": PLUGINSD | STREAMING;
    End = "END": PLUGINSD | STREAMING | DATA;
    End2 = "END2": STREAMING | DATA;
    Exit = "EXIT": PLUGINSD;
    Flush = "FLUSH": PLUGINSD;
    Function = "FUNCTION": PLUGINSD | STREAMING | METADATA;
    FunctionDel = "FUNCTION_DEL": PLUGINSD | STREAMING | METADATA;
    FunctionProgress = "FUNCTION_PROGRESS": PLUGINSD | STREAMING;
    FunctionResultBegin = "FUNCTION_RESULT_BEGIN": PLUGINSD | STREAMING;
    Host = "HOST": PLUGINSD | METADATA;
    HostDefine = "HOST_DEFINE": PLUGINSD | METADATA;
    HostDefineEnd = "HOST_DEFINE_END": PLUGINSD | METADATA;
    HostLabel = "HOST_LABEL": PLUGINSD | METADATA;
    Json = "JSON": STREAMING | METADATA;
    Label = "LABEL": PLUGINSD | STREAMING | METADATA;
    Overwrite = "OVERWRITE": PLUGINSD | STREAMING | METADATA;
    PluginKeepalive = "PLUGIN_KEEPALIVE": PLUGINSD;
    Rbegin = "RBEGIN": STREAMING | REPLICATION | METADATA;
    Rdstate = "RDSTATE": STREAMING | REPLICATION | METADATA;
    Rend = "REND": STREAMING | REPLICATION | METADATA;
    ReportJobStatus = "REPORT_JOB_STATUS": PLUGINSD | STREAMING;
    Rset = "RSET": STREAMING | REPLICATION | DATA;
    Rsstate = "RSSTATE": STREAMING | REPLICATION | METADATA;
    Set = "SET": PLUGINSD | STREAMING | DATA;
    Set2 = "SET2": STREAMING | DATA;
    TrustDurations = "TRUST_DURATIONS": PLUGINSD | METADATA;
    Variable = "VARIABLE": PLUGINSD | STREAMING | METADATA;
}

impl Keyword {
    /// The exact, case-sensitive match gperf performs.
    pub fn lookup(word: &[u8]) -> Option<Keyword> {
        TABLE
            .iter()
            .find(|(_, name, _)| name.as_bytes() == word)
            .map(|(k, _, _)| *k)
    }

    pub fn name(self) -> &'static str {
        TABLE
            .iter()
            .find(|(k, _, _)| *k == self)
            .map_or("", |(_, name, _)| name)
    }

    pub fn repertoire(self) -> Repertoire {
        TABLE
            .iter()
            .find(|(k, _, _)| *k == self)
            .map_or(Repertoire(0), |(_, _, r)| *r)
    }

    /// Replication-only keywords, whose traffic streaming accounts separately
    /// (`PARSER_REP_REPLICATION` without `PARSER_REP_DATA`).
    pub fn is_replication_metadata(self) -> bool {
        let r = self.repertoire();
        r.contains(Repertoire::REPLICATION) && !r.contains(Repertoire::DATA)
    }

    pub fn all() -> impl Iterator<Item = Keyword> {
        TABLE.iter().map(|(k, _, _)| *k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookups_are_exact() {
        assert_eq!(Keyword::lookup(b"BEGIN2"), Some(Keyword::Begin2));
        assert_eq!(Keyword::lookup(b"begin2"), None);
        assert_eq!(Keyword::lookup(b"FUNCTION_RESULT_END"), None);
        assert_eq!(Keyword::all().count(), 41);
        assert!(Keyword::Set2.repertoire().contains(Repertoire::STREAMING));
        assert!(!Keyword::Set2.repertoire().contains(Repertoire::PLUGINSD));
        assert!(Keyword::Rbegin.is_replication_metadata());
        assert!(!Keyword::Rset.is_replication_metadata());
    }
}
