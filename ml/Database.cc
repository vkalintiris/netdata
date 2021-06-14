// SPDX-License-Identifier: GPL-3.0-or-later

#include "Database.h"

const char *ml::Database::SQL_CREATE_ANOMALIES_TABLE =
    "CREATE TABLE IF NOT EXISTS anomaly_events( "
    "     anomaly_detector_name text NOT NULL, "
    "     anomaly_detector_version int NOT NULL, "
    "     host_id blob NOT NULL, "
    "     after int NOT NULL, "
    "     before int NOT NULL, "
    "     anomaly_event_info text, "
    "     PRIMARY KEY( "
    "         anomaly_detector_name, anomaly_detector_version, "
    "         host_id, after, before "
    "     ) "
    ");";

const char *ml::Database::SQL_INSERT_ANOMALY =
    "INSERT INTO anomaly_events( "
    "     anomaly_detector_name, anomaly_detector_version, "
    "     host_id, after, before, anomaly_event_info) "
    "VALUES (?1, ?2, ?3, ?4, ?5, ?6);";

const char *ml::Database::SQL_SELECT_ANOMALY_EVENTS =
    "SELECT after, before FROM anomaly_events WHERE"
    "   anomaly_detector_name == ?1 AND"
    "   anomaly_detector_version == ?2 AND"
    "   host_id == ?3 AND"
    "   after >= ?4 AND"
    "   before <= ?5;";

using namespace ml;

bool Statement::prepare(sqlite3 *Conn) {
    if (ParsedStmt)
        return true;

    int RC = sqlite3_prepare_v2(Conn, RawStmt, -1, &ParsedStmt, nullptr);
    if (RC == SQLITE_OK)
        return true;

    std::string Msg = "Statement \"%s\" preparation failed due to \"%s\"";
    error(Msg.c_str(), RawStmt, sqlite3_errstr(RC));

    return false;
}

bool Statement::bind(size_t Pos, const std::string &Value) {
    int RC = sqlite3_bind_text(ParsedStmt, Pos, Value.c_str(), -1, SQLITE_TRANSIENT);
    if (RC == SQLITE_OK)
        return true;

    std::string Msg = "Failed to bind text '%s' (pos = %zu) in statement '%s'.";
    error(Msg.c_str(), Value.c_str(), Pos, RawStmt);
    return false;
}

bool Statement::bind(size_t Pos, int Value) {
    int RC = sqlite3_bind_int(ParsedStmt, Pos, Value);
    if (RC == SQLITE_OK)
        return true;

    std::string Msg = "Failed to bind integer %d (pos = %zu) in statement '%s'.";
    error(Msg.c_str(), Value, Pos, RawStmt);
    return false;
}

bool Statement::bind(size_t Pos, const uuid_t Value) {
    int RC = sqlite3_bind_blob(ParsedStmt, Pos, Value, sizeof(*Value), SQLITE_TRANSIENT);
    if (RC == SQLITE_OK)
        return true;

    char UUIDStr[UUID_STR_LEN];
    uuid_unparse_lower(Value, UUIDStr);

    std::string Msg = "Failed to bind uuid_t %s (pos = %zu) in statement '%s'.";
    error(Msg.c_str(), UUIDStr, Pos, RawStmt);
    return false;
}

bool Statement::bind(size_t Pos, const nlohmann::json &Value) {
    std::string JsonString = Value.dump(4);

    int RC = sqlite3_bind_text(ParsedStmt, Pos, JsonString.c_str(), -1, SQLITE_TRANSIENT);
    if (RC == SQLITE_OK)
        return true;

    std::string Msg = "Failed to bind json (pos = %zu) in statement '%s'.";
    error(Msg.c_str(), Pos, RawStmt);
    return false;
}

bool Statement::resetAndClear(bool Ret) {
    int RC = sqlite3_reset(ParsedStmt);
    if (RC != SQLITE_OK) {
        error("Could not reset statement: '%s'", RawStmt);
        return false;
    }

    RC = sqlite3_clear_bindings(ParsedStmt);
    if (RC != SQLITE_OK) {
        error("Could not clear bindings in statement: '%s'", RawStmt);
        return false;
    }

    return Ret;
}

bool Statement::exec(sqlite3 *Conn, std::string AnomalyDetectorName,
                                    int AnomalyDetectorVersion,
                                    uuid_t HostUUID,
                                    time_t After,
                                    time_t Before,
                                    const nlohmann::json &Json)
{
    if (!prepare(Conn))
        return false;

    size_t numSuccessfulBindings = bind(1, AnomalyDetectorName) +
                                   bind(2, AnomalyDetectorVersion) +
                                   bind(3, HostUUID) +
                                   bind(4, After) +
                                   bind(5, Before) +
                                   bind(6, Json);

    switch (numSuccessfulBindings) {
    case 6:
        break;
    case 0:
        return false;
    default:
        return resetAndClear(false);
    }

    while (true) {
        switch (int RC = sqlite3_step(ParsedStmt)) {
        case SQLITE_BUSY:
        case SQLITE_LOCKED:
            usleep(SQLITE_INSERT_DELAY * USEC_PER_MS);
            continue;
        case SQLITE_DONE:
            return resetAndClear(true);
        default:
            error("Stepping through '%s' returned rc=%d", RawStmt, RC);
            return resetAndClear(false);
        }
    }
}

bool Statement::exec(sqlite3 *Conn, std::vector<std::pair<time_t, time_t>> &TimeRanges,
                                    std::string AnomalyDetectorName,
                                    int AnomalyDetectorVersion,
                                    uuid_t HostUUID,
                                    time_t After,
                                    time_t Before)
{
    if (!prepare(Conn))
        return false;

    size_t numSuccessfulBindings = bind(1, AnomalyDetectorName) +
                                   bind(2, AnomalyDetectorVersion) +
                                   bind(3, HostUUID) +
                                   bind(4, After) +
                                   bind(5, Before);

    switch (numSuccessfulBindings) {
    case 0:
        return false;
    case 5:
        break;
    default:
        return resetAndClear(false);
    }

    while (true) {
        switch (int RC = sqlite3_step(ParsedStmt)) {
        case SQLITE_BUSY:
        case SQLITE_LOCKED:
            usleep(SQLITE_INSERT_DELAY * USEC_PER_MS);
            continue;
        case SQLITE_ROW: {
            time_t After = sqlite3_column_int64(ParsedStmt, 0);
            time_t Before = sqlite3_column_int64(ParsedStmt, 1);
            TimeRanges.push_back({After, Before});
            continue;
        }
        case SQLITE_DONE:
            return resetAndClear(true);
        default:
            error("Stepping through '%s' returned rc=%d", RawStmt, RC);
            return resetAndClear(false);
        }
    }
}

Database::Database(const std::string Path) {
    // Get sqlite3 connection handle.
    int RC = sqlite3_open(Path.c_str(), &Conn);
    if (RC != SQLITE_OK) {
        std::string Msg = "Failed to initialize ML DB at %s, due to \"%s\"";
        error(Msg.c_str(), Path.c_str(), sqlite3_errstr(RC));
        Conn = nullptr;
        return;
    }

    // Create anomaly events table if it does not exist.
    char *ErrMsg;
    RC = sqlite3_exec(Conn, SQL_CREATE_ANOMALIES_TABLE, 0, 0, &ErrMsg);
    if (RC == SQLITE_OK)
        return;

    error("SQLite error during database initialization, rc = %d (%s)", RC, ErrMsg);
    error("SQLite failed statement: %s", SQL_CREATE_ANOMALIES_TABLE);
    sqlite3_free(ErrMsg);
    sqlite3_close(Conn);
    Conn = nullptr;
}
