// SPDX-License-Identifier: GPL-3.0-or-later

#include "Database.h"

using namespace ml;

static const char *SQL_CREATE_ANOMALY_EVENTS_TABLE =
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

static const char *SQL_INSERT_INTO_ANOMALY_EVENTS_TABLE =
    "INSERT INTO anomaly_events( "
    "     anomaly_detector_name, anomaly_detector_version, "
    "     host_id, after, before, anomaly_event_info) "
    "VALUES (?1, ?2, ?3, ?4, ?5, ?6);";

Database::Database(const std::string Path) {
    const char *CreateTableStr = SQL_CREATE_ANOMALY_EVENTS_TABLE;
    const char *InsertIntoStr = SQL_INSERT_INTO_ANOMALY_EVENTS_TABLE;

    // Get sqlite3 connection handle.
    int RC = sqlite3_open(Path.c_str(), &Conn);
    if (RC != SQLITE_OK) {
        std::string Msg = "Failed to initialize ML DB at %s, due to \"%s\"";
        error(Msg.c_str(), Path.c_str(), sqlite3_errstr(RC));
        goto CONN_ERROR;
    }

    // Create anomaly events table.
    char *ErrMsg;
    RC = sqlite3_exec(Conn, CreateTableStr, 0, 0, &ErrMsg);
    if (RC != SQLITE_OK) {
        error("SQLite error during database initialization, rc = %d (%s)", RC, ErrMsg);
        error("SQLite failed statement: %s", CreateTableStr);
        sqlite3_free(ErrMsg);
        goto EXEC_ERROR;
    }

    // Prepare insert statement.
    RC = sqlite3_prepare_v2(Conn, InsertIntoStr, -1, &InsertStmt, nullptr);
    if (RC != SQLITE_OK) {
        std::string Msg = "Statement \"%s\" preparation failed due to \"%s\"";
        error(Msg.c_str(), InsertIntoStr, sqlite3_errstr(RC));
        goto PREP_ERROR;
    }

    // Everything went fine. We have a connection and a prepared statement.
    return;

CONN_ERROR:
    Conn = nullptr;
EXEC_ERROR:
PREP_ERROR:
    InsertStmt = nullptr;
}

bool Database::bind(size_t Pos, const std::string &Value) {
    int RC = sqlite3_bind_text(InsertStmt, Pos, Value.c_str(), -1, SQLITE_TRANSIENT);
    if (RC == SQLITE_OK)
        return true;

    std::string Msg = "Failed to bind text '%s' (pos = %zu) in statement '%s'.";
    error(Msg.c_str(), Value.c_str(), Pos, SQL_INSERT_INTO_ANOMALY_EVENTS_TABLE);
    return false;
}

bool Database::bind(size_t Pos, int Value) {
    int RC = sqlite3_bind_int(InsertStmt, Pos, Value);
    if (RC == SQLITE_OK)
        return true;

    std::string Msg = "Failed to bind integer %d (pos = %zu) in statement '%s'.";
    error(Msg.c_str(), Value, Pos, SQL_INSERT_INTO_ANOMALY_EVENTS_TABLE);
    return false;
}

bool Database::bind(size_t Pos, const uuid_t Value) {
    int RC = sqlite3_bind_blob(InsertStmt, Pos, Value, sizeof(*Value), SQLITE_TRANSIENT);
    if (RC == SQLITE_OK)
        return true;

    char UUIDStr[UUID_STR_LEN];
    uuid_unparse_lower(Value, UUIDStr);

    std::string Msg = "Failed to bind uuid_t %s (pos = %zu) in statement '%s'.";
    error(Msg.c_str(), UUIDStr, Pos, SQL_INSERT_INTO_ANOMALY_EVENTS_TABLE);
    return false;
}

bool Database::bind(size_t Pos, const nlohmann::json &Value) {
    std::string JsonString = Value.dump(4);

    int RC = sqlite3_bind_text(InsertStmt, Pos, JsonString.c_str(), -1, SQLITE_TRANSIENT);
    if (RC == SQLITE_OK)
        return true;

    std::string Msg = "Failed to bind json (pos = %zu) in statement '%s'.";
    error(Msg.c_str(), Pos, SQL_INSERT_INTO_ANOMALY_EVENTS_TABLE);
    return false;
}

bool Database::step() {
    int RC;

    while ((RC = sqlite3_step(InsertStmt)) != SQLITE_DONE) {
        if (RC == SQLITE_BUSY || RC == SQLITE_LOCKED) {
            usleep(SQLITE_INSERT_DELAY * USEC_PER_MS);
            continue;
        }

        error("Failed to insert new anomaly event in SQLite: rc=%d", RC);
        return false;
    }

    RC = sqlite3_reset(InsertStmt);
    if (RC != SQLITE_OK) {
        error("Could not reset insert statement.");
        return false;
    }

    RC = sqlite3_clear_bindings(InsertStmt);
    if (RC != SQLITE_OK) {
        error("Could not clear bindings of insert statement.");
        return false;
    }

    return true;
}

bool Database::insertIntoAnomalyEvents(std::string AnomalyDetectorName,
                                       int AnomalyDetectorVersion,
                                       uuid_t HostUUID,
                                       time_t After,
                                       time_t Before,
                                       const nlohmann::json &Json)
{
    if (!InsertStmt)
        return false;

    bool bindFailed = bind(1, AnomalyDetectorName) &&
                      bind(2, AnomalyDetectorVersion) &&
                      bind(3, HostUUID) &&
                      bind(4, After) &&
                      bind(5, Before) &&
                      bind(6, Json);

    bool stepFailed = step();

    return bindFailed && stepFailed;
}
