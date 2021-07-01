#ifndef ML_TRACER_H
#define ML_TRACER_H

#include "Config.h"

namespace ml {

class Tracer {
public:
    Tracer(const char *Thread, const char *Name, const char *Key, const char *Value)
        : Thread(Thread), Name(Name) {
        SPDR_BEGIN1(ml::Cfg.SpdrCtx, Thread, Name, SPDR_STR(Key, Value));
    }

    virtual ~Tracer() {
        SPDR_END(ml::Cfg.SpdrCtx, Thread, Name);
    }

private:
    const char *Thread;
    const char *Name;
};

class ReportableTracer : Tracer {
public:
    ReportableTracer(const char *Thread, const char *Name, const char *Key, const char *Value)
        : Tracer(Thread, Name, Key, Value) {}

    virtual ~ReportableTracer() {
        spdr_report(
            Cfg.SpdrCtx, SPDR_CHROME_REPORT,
            [](const char *data, void *user_data) {
                (void) user_data;
                fputs(data, Cfg.LogFP);
            },
            nullptr
        );

        fflush(Cfg.LogFP);
        fclose(Cfg.LogFP);
    }
};

} // namespace ml

#endif /* ML_TRACER_H */
