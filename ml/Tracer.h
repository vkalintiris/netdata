#ifndef ML_TRACER_H
#define ML_TRACER_H

#include "Config.h"

namespace ml {

class Tracer {
public:
    Tracer(const char *Thread, const char *Name)
        : Thread(Thread), Name(Name) {
        SPDR_BEGIN(ml::Cfg.SpdrCtx, Thread, Name);
    }

    Tracer(const char *Thread, const char *Name, const char *Key, const char *Value)
        : Thread(Thread), Name(Name) {
        SPDR_BEGIN1(ml::Cfg.SpdrCtx, Thread, Name, SPDR_STR(Key, Value));
    }

    Tracer(const char *Thread, const char *Name, const char *Key, size_t Value)
        : Thread(Thread), Name(Name) {
        SPDR_BEGIN1(ml::Cfg.SpdrCtx, Thread, Name, SPDR_INT(Key, Value));
    }

    virtual ~Tracer() {
        SPDR_END(ml::Cfg.SpdrCtx, Thread, Name);
    }

private:
    const char *Thread;
    const char *Name;
};

} // namespace ml

#endif /* ML_TRACER_H */
