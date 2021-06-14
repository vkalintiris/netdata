// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_DATABASE_H
#define ML_DATABASE_H

#include "ml-private.h"
#include "Unit.h"
#include "json.hpp"

namespace ml {

class Statement {
public:
    Statement(const char *RawStmt) : RawStmt(RawStmt), ParsedStmt(nullptr) {}

    bool exec(sqlite3 *Conn, std::string AnomalyDetectorName,
                             int AnomalyDetectorVersion,
                             uuid_t HostUUID,
                             time_t After,
                             time_t Before,
                             const nlohmann::json &Json);

    bool exec(sqlite3 *Conn, std::vector<std::pair<time_t, time_t>> &TimeRanges,
                             std::string AnomalyDetectorName,
                             int AnomalyDetectorVersion,
                             uuid_t HostUUID,
                             time_t After,
                             time_t Before);

private:
    bool prepare(sqlite3 *Conn);

    bool bind(size_t Pos, const std::string &Value);
    bool bind(size_t Pos, int Value);
    bool bind(size_t Pos, const uuid_t Value);
    bool bind(size_t Pos, const nlohmann::json &Value);

    bool resetAndClear(bool Ret);

private:
    const char *RawStmt;
    sqlite3_stmt *ParsedStmt;
};

class Database {
private:
    static const char *SQL_CREATE_ANOMALIES_TABLE;
    static const char *SQL_INSERT_ANOMALY;
    static const char *SQL_SELECT_ANOMALY_EVENTS;

public:
    Database(const std::string Path);

    template<class ...ArgTypes>
    bool insertAnomaly(ArgTypes&&... Args) {
        if (!Conn)
            return false;

        return InsertAnomalyStmt.exec(Conn, std::forward<ArgTypes>(Args)...);
    }

    template<class ...ArgTypes>
    bool getAnomaliesInRange(ArgTypes&&... Args) {
        if (!Conn)
            return false;

        return GetAnomaliesInRangeStmt.exec(Conn, std::forward<ArgTypes>(Args)...);
    }

private:
    sqlite3 *Conn;

    Statement InsertAnomalyStmt{SQL_INSERT_ANOMALY};
    Statement GetAnomaliesInRangeStmt{SQL_SELECT_ANOMALY_EVENTS};
};

}

#endif /* ML_DATABASE_H */
