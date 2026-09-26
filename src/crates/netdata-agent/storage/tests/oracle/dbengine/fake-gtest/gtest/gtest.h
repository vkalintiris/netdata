// SPDX-License-Identifier: GPL-3.0-or-later
//
// Minimal stand-in for <gtest/gtest.h>, just enough to compile src/database/engine/page_test.cc VERBATIM against the
// real page.c and record what every EXPECT_* evaluates to. It is not googletest:
//   - every TEST runs in a forked child, so a fatal() (exit(1)) or a crash ends only that test;
//   - EXPECT_* results are aggregated per source line in MAP_SHARED memory, so they survive the child's death;
//   - EXPECT_DEATH forks again and passes when the statement exits non-zero or is killed by a signal;
//   - the parent prints one JSON object per test on stdout (JSON lines).
// Environment: FAKE_GTEST_FILTER=Suite.Name,Suite.Name (optional) limits the tests that run;
// FAKE_GTEST_TIMEOUT=seconds (default 300) kills a test that does not finish (reported as signal 14, SIGALRM).

#ifndef FAKE_GTEST_H
#define FAKE_GTEST_H

#include <cctype>
#include <cmath>
#include <csignal>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <type_traits>
#include <vector>

#include <fcntl.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

namespace fake_gtest {

struct Slot {
    int line;
    char macro[16];
    char args[200];
    uint64_t evals;
    uint64_t passed;
    uint64_t failed;
    char first_failure[240];
    char last_value[240];
};

struct Shared {
    int nslots;
    int last_line;
    Slot slots[128];
};

struct Test {
    const char *name;
    int line;
    void (*body)();
};

inline std::vector<Test> &registry() {
    static std::vector<Test> r;
    return r;
}

inline Shared *&shared() {
    static Shared *s = nullptr;
    return s;
}

struct Registrar {
    Registrar(const char *name, int line, void (*body)()) { registry().push_back(Test{name, line, body}); }
};

template <typename T>
inline typename std::enable_if<std::is_floating_point<T>::value, std::string>::type fmt(const T &v) {
    char b[64];
    snprintf(b, sizeof(b), "%.17g", (double)v);
    return b;
}

template <typename T>
inline typename std::enable_if<std::is_integral<T>::value, std::string>::type fmt(const T &v) {
    char b[64];
    if (std::is_signed<T>::value)
        snprintf(b, sizeof(b), "%lld", (long long)v);
    else
        snprintf(b, sizeof(b), "%llu", (unsigned long long)v);
    return b;
}

template <typename T>
inline typename std::enable_if<std::is_pointer<T>::value, std::string>::type fmt(const T &v) {
    if ((uintptr_t)v == (uintptr_t)-1)
        return "PGD_EMPTY";
    if (!v)
        return "NULL";
    return "non-null-pointer";
}

template <typename T>
inline typename std::enable_if<!std::is_floating_point<T>::value && !std::is_integral<T>::value &&
                                   !std::is_pointer<T>::value,
                               std::string>::type
fmt(const T &) {
    return "<object>";
}

inline void json_str(FILE *f, const char *s) {
    fputc('"', f);
    for (; *s; s++) {
        unsigned char c = (unsigned char)*s;
        if (c == '"' || c == '\\')
            fprintf(f, "\\%c", c);
        else if (c < 0x20)
            fprintf(f, "\\u%04x", c);
        else
            fputc(c, f);
    }
    fputc('"', f);
}

inline void record(int line, const char *macro, const char *args, bool ok, const std::string &detail) {
    Shared *s = shared();
    if (!s)
        return;
    s->last_line = line;
    Slot *slot = nullptr;
    for (int i = 0; i < s->nslots; i++)
        if (s->slots[i].line == line && !strcmp(s->slots[i].macro, macro)) {
            slot = &s->slots[i];
            break;
        }
    if (!slot) {
        if (s->nslots == 128)
            return;
        slot = &s->slots[s->nslots++];
        memset(slot, 0, sizeof(*slot));
        slot->line = line;
        snprintf(slot->macro, sizeof(slot->macro), "%s", macro);
        snprintf(slot->args, sizeof(slot->args), "%s", args);
    }
    slot->evals++;
    if (ok)
        slot->passed++;
    else {
        if (!slot->failed)
            snprintf(slot->first_failure, sizeof(slot->first_failure), "%s", detail.c_str());
        slot->failed++;
    }
    snprintf(slot->last_value, sizeof(slot->last_value), "%s", detail.c_str());
}

template <typename A, typename B>
inline void expect_eq(int line, const char *args, const A &a, const B &b) {
    bool ok = (a == b);
    record(line, "EXPECT_EQ", args, ok, "lhs=" + fmt(a) + " rhs=" + fmt(b));
}

template <typename A, typename B, typename C>
inline void expect_near(int line, const char *args, const A &a, const B &b, const C &err) {
    double da = (double)a, db = (double)b, de = (double)err;
    bool ok = std::fabs(da - db) <= de;
    record(line, "EXPECT_NEAR", args, ok, "val1=" + fmt(da) + " val2=" + fmt(db) + " abs_error=" + fmt(de));
}

inline void expect_bool(int line, const char *macro, const char *args, bool v, bool want) {
    record(line, macro, args, v == want, std::string("value=") + (v ? "true" : "false"));
}

template <typename F>
inline void expect_death(int line, const char *args, F stmt) {
    fflush(nullptr);
    pid_t pid = fork();
    if (pid == 0) {
        int fd = open("/dev/null", O_WRONLY);
        if (fd >= 0)
            dup2(fd, 2);
        alarm(60);
        stmt();
        _exit(0);
    }
    int status = 0;
    waitpid(pid, &status, 0);
    bool died = WIFSIGNALED(status) || (WIFEXITED(status) && WEXITSTATUS(status) != 0);
    char b[128];
    if (WIFSIGNALED(status))
        snprintf(b, sizeof(b), "statement killed by signal %d", WTERMSIG(status));
    else
        snprintf(b, sizeof(b), "statement exited with status %d", WEXITSTATUS(status));
    record(line, "EXPECT_DEATH", args, died, b);
}

inline bool selected(const char *name) {
    const char *f = getenv("FAKE_GTEST_FILTER");
    if (!f || !*f)
        return true;
    std::string all = std::string(",") + f + ",";
    return all.find(std::string(",") + name + ",") != std::string::npos;
}

inline int run_all() {
    Shared *s = (Shared *)mmap(nullptr, sizeof(Shared), PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
    if (s == MAP_FAILED) {
        perror("mmap");
        return 2;
    }
    shared() = s;
    int rc = 0;
    for (const Test &t : registry()) {
        if (!selected(t.name))
            continue;
        memset(s, 0, sizeof(*s));
        FILE *err = tmpfile();
        fflush(nullptr);
        pid_t pid = fork();
        if (pid == 0) {
            dup2(fileno(err), 2);
            const char *tmo = getenv("FAKE_GTEST_TIMEOUT");
            alarm(tmo && *tmo ? (unsigned)atoi(tmo) : 300);
            t.body();
            fflush(nullptr);
            _exit(0);
        }
        int status = 0;
        waitpid(pid, &status, 0);

        std::string stderr_text;
        if (err) {
            rewind(err);
            char buf[4096];
            size_t n;
            while ((n = fread(buf, 1, sizeof(buf), err)) > 0)
                stderr_text.append(buf, n);
            fclose(err);
        }
        // keep only the msg="..." fields of netdata's log lines (drops timestamps and thread ids) and mask the
        // thread-dependent ARAL partition number, so the output is reproducible
        std::string msgs;
        for (size_t at = 0; (at = stderr_text.find("msg=\"", at)) != std::string::npos;) {
            size_t end = stderr_text.find("\"\n", at + 5);
            if (end == std::string::npos)
                end = stderr_text.size();
            msgs += (msgs.empty() ? "" : "\n") + stderr_text.substr(at + 5, end - at - 5);
            at = end;
        }
        if (!msgs.empty())
            stderr_text = msgs;
        for (size_t at = 0; (at = stderr_text.find("partition: ", at)) != std::string::npos;) {
            at += 11;
            size_t end = at;
            while (end < stderr_text.size() && isdigit((unsigned char)stderr_text[end]))
                end++;
            stderr_text.replace(at, end - at, "N");
        }
        if (stderr_text.size() > 600)
            stderr_text = "..." + stderr_text.substr(stderr_text.size() - 600);

        uint64_t failed = 0;
        for (int i = 0; i < s->nslots; i++)
            failed += s->slots[i].failed;

        const char *outcome;
        if (WIFSIGNALED(status) || (WIFEXITED(status) && WEXITSTATUS(status) != 0))
            outcome = "died";
        else if (failed)
            outcome = "failed";
        else
            outcome = "passed";
        if (strcmp(outcome, "passed"))
            rc = 1;

        FILE *o = stdout;
        fprintf(o, "{\"test\":");
        json_str(o, t.name);
        fprintf(o, ",\"source_line\":%d,\"outcome\":\"%s\"", t.line, outcome);
        if (WIFSIGNALED(status))
            fprintf(o, ",\"signal\":%d", WTERMSIG(status));
        else
            fprintf(o, ",\"exit_status\":%d", WEXITSTATUS(status));
        fprintf(o, ",\"last_expect_line\":%d,\"stderr_tail\":", s->last_line);
        json_str(o, stderr_text.c_str());
        fprintf(o, ",\"expects\":[");
        for (int i = 0; i < s->nslots; i++) {
            Slot *sl = &s->slots[i];
            fprintf(o, "%s{\"line\":%d,\"macro\":", i ? "," : "", sl->line);
            json_str(o, sl->macro);
            fprintf(o, ",\"args\":");
            json_str(o, sl->args);
            fprintf(o, ",\"evaluations\":%llu,\"passed\":%llu,\"failed\":%llu,\"last_value\":",
                    (unsigned long long)sl->evals, (unsigned long long)sl->passed, (unsigned long long)sl->failed);
            json_str(o, sl->last_value);
            if (sl->failed) {
                fprintf(o, ",\"first_failure\":");
                json_str(o, sl->first_failure);
            }
            fprintf(o, "}");
        }
        fprintf(o, "]}\n");
        fflush(o);
    }
    return rc;
}

} // namespace fake_gtest

namespace testing {
inline void InitGoogleTest(int *, char **) {}
} // namespace testing

#define RUN_ALL_TESTS() fake_gtest::run_all()

#define TEST(suite, name)                                                                                              \
    static void suite##_##name##_body();                                                                               \
    static fake_gtest::Registrar suite##_##name##_registrar(#suite "." #name, __LINE__, &suite##_##name##_body);       \
    static void suite##_##name##_body()

#define EXPECT_EQ(a, b) fake_gtest::expect_eq(__LINE__, #a ", " #b, (a), (b))
#define EXPECT_NEAR(a, b, e) fake_gtest::expect_near(__LINE__, #a ", " #b ", " #e, (a), (b), (e))
#define EXPECT_TRUE(c) fake_gtest::expect_bool(__LINE__, "EXPECT_TRUE", #c, (bool)(c), true)
#define EXPECT_FALSE(c) fake_gtest::expect_bool(__LINE__, "EXPECT_FALSE", #c, (bool)(c), false)
#define EXPECT_DEATH(stmt, re) fake_gtest::expect_death(__LINE__, #stmt, [&]() { stmt; })

#endif // FAKE_GTEST_H
