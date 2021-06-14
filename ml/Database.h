// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_DATABASE_H
#define ML_DATABASE_H

#include "ml-private.h"
#include "Unit.h"
#include "json.hpp"

namespace ml {

class Database {
public:
    Database(const std::string Path);

    bool insertIntoAnomalyEvents(std::string AnomalyDetectorName,
                                 int AnomalyDetectorVersion,
                                 uuid_t HostUUID,
                                 time_t After,
                                 time_t Before,
                                 const nlohmann::json &Json);

private:
    bool bind(size_t Pos, const std::string &Value);
    bool bind(size_t Pos, int Value);
    bool bind(size_t Pos, const uuid_t Value);
    bool bind(size_t Pos, const nlohmann::json &Value);

    bool step();

private:
    const std::string Path;
    sqlite3 *Conn;
    sqlite3_stmt *InsertStmt;
};

}

#endif /* ML_DATABASE_H */
