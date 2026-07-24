# Children agents OOM — investigation report and fix handoff

Date: 2026-07-24
Investigated on: `~/repos/nd/oom` (dedicated worktree, branch `oom`, HEAD `92c1988f95`, v2.10.0-883)
Status: root causes identified and reproduced locally; fixes not yet written.

This document is a complete handoff: it explains the production symptom, the
investigation method, the exact defects with commits and code locations, the
evidence behind each claim, a reproduction recipe, and guidance for the fixes
and their validation. It is written for both a human engineer and an AI
assistant starting a fresh session.

---

## 1. The production symptom

DevOps reported that Netdata child agents in the staging EKS cluster started
getting OOM-killed "this or last week" (mid-July 2026). A Netdata Cloud AI
Insights report (`~/mo/oom-inv.pdf`, report id
`9385ef0c-b8ae-4f49-a0e0-3c00d31d5e2f`, generated 2026-07-24 10:10) added
crucial quantitative facts, though its narrative conclusions must be read with
care (see section 3).

The staging deployment shape, from the infra repo
(`infra/aws/org/us-east-1/staging/netdata/latest.tpl`): child agents run as a
DaemonSet with `[db] mode = ram`, `update every = 1`, `retention = 7200`, ML
disabled, health mostly disabled, streaming to a parent Deployment
(`mode = dbengine`), image `netdata/netdata:latest` — i.e. **the nightly
build, re-pulled by a rolling deploy every day at ~06:00 UTC**. The child
config carries this cgroups filter, which matters a lot later:

    [plugin:cgroups]
    enable by default cgroups names matching = !*cronjob* *

Reliable facts from the Cloud report (its chart screenshots, which come from
real `netdata.memory` data):

- Child pods grow **linearly at 65–191 MiB/hour for 12–24 hours** after every
  daily deploy, then get OOM-killed by the kernel (`CONSTRAINT_MEMCG`,
  `memory.oom.group` kills the whole container). Restart resets the cycle —
  a daily sawtooth.
- On an affected child, the dominant *growing* category in `netdata.memory`
  is **`rrd`** (~4.5 → 6 GiB across a cycle), while **`dbengine` is 0 bytes**
  (children run `mode=ram`; dbengine is not instantiated).
- Chart/dimension counts on the children stay **flat** while memory grows.
- The parent also leaks, slower (~20–21h to OOM), with a metadata-heavy
  profile (`metadata` ≈ 7.5 GiB in its screenshot).
- The two non-cluster control nodes (`agent-events` on v2.10.0-738-nightly,
  `packages` on v2.10.4) are flat. Caveat: both are standalone nodes without
  the churn workload, so they are weak controls.

The regression window implied by the versions: **nightly v2.10.0-738 (built
2026-07-12, changelog commit `97dcee869d`) is "good", nightly v2.10.0-858
(built 2026-07-23, changelog commit `7205ce81ae`) is "bad"** — i.e. commits
merged between 2026-07-12 and 2026-07-22. One caution: because the fleet
redeploys `:latest` daily, "all leaking nodes run -858" is circular and does
NOT itself date the regression; only the date of the *first* OOM kill can do
that (still unconfirmed, see section 8).

## 2. Investigation method (so results can be re-derived)

Everything was done locally on the dev box, no staging access.

**The rig.** One parent plus four children, each child built from a different
point in history, all running simultaneously against the same parent with
identical configuration mirroring staging (`mode=ram`, `retention=7200`,
`update every=1`, ML off, streaming on, and later
`cleanup obsolete charts after = 60` to accelerate the obsolete-chart
cleanup horizon from its 3600s default — default defined at
`src/daemon/config/netdata-conf-db.c:14`):

| child | version | source | commit date of tip |
|-------|---------|--------|--------------------|
| HEAD  | v2.10.0-883 | this worktree (`92c1988f95`) | 2026-07-24 |
| 813   | v2.10.0-813 | `~/opt/netdata-builds/master` (`92d3d91b7b`) | 2026-07-19 |
| 808   | v2.10.0-808 | `~/opt/netdata-builds/otel-events` (`58b22aa7be`) | 2026-07-18 |
| 696   | v2.10.0-696 | `~/opt/netdata-builds/sjr-json` (`fa8bc7161b`, feature branch off master ~Jul 5) | 2026-07-05 |

Each instance got its own `NETDATA_RUN_DIR` (unix-socket paths must be short,
and the spawn-server socket collides between instances otherwise — symptom:
`SPAWN SERVER: Server is already listening on path ...`).

**The workload.** Two external plugins (pluginsd protocol over stdout):

- `synthetic.plugin`: a constant load of 2000 charts × 5 dimensions, 1s
  updates — approximates a busy k8s node so the children sit at a realistic
  ~12k dimensions (~400 MB of ram-mode reservations).
- `churn.plugin`: every 60s creates 25 brand-new charts (2 dims each), feeds
  them for 45s, then marks them **obsolete** and never touches them again.
  This mimics cronjob pods on a k8s node. It has two modes controlled by env
  var `OBSOLETE_DIMS`: `0` (default) re-sends only the CHART line with the
  `obsolete` option — exactly what cgroups.plugin and go.d do (chart-level
  obsoletion); `1` also marks every DIMENSION line `obsolete`
  (dimension-level obsoletion). The difference between these two modes is
  what exposed defect 1.

Full plugin sources are in appendix A.

**The measurements.** Three independent channels, cross-checked:

1. Netdata's own accounting: the `rrd` dimension of the `netdata.memory`
   chart (`pulse_db_rrd_memory`). In `mode=ram` this counter is incremented
   with the ring-buffer size when a dimension is created
   (`src/database/rrddim.c:81-84`: each dimension anonymously mmaps
   `retention × sizeof(storage_number)` = 7200×4 = 28.8 KB) and decremented
   only when the ring is actually freed. It is the same counter the Cloud
   report shows growing on staging children.
2. Kernel ground truth: `VmData` from `/proc/<pid>/status`. Because each
   ring is its own anonymous mmap, retained rings grow VmData at the churn
   rate regardless of page-touching. (RSS lags: pages fault in lazily, one
   4KB page per dimension per ~1024 seconds, so RSS climbs in ~17-minute
   staircase waves toward the full reservation.)
3. Netdata's worker job counters (needs `[pulse] extended = yes`): the
   `netdata.workers_jobs_by_type_libuv` chart exposes per-job counts of the
   cleanup machinery — `cleanup obsolete charts`, `archive chart dimensions`,
   `archive dimension`, `free dimension`, `free chart`. Whether `free chart`
   ever ticks is the direct answer to "does cleanup work".

**Instrumented builds.** Two temporary diagnostic patches (still present in
the worktree, see section 9) were added to attribute the leak:

- `src/database/ram/rrddim_mem.c`: track acquire/release call sites
  (`__builtin_return_address(0)`) of ram-engine metric handles, and log
  every case where a handle survives `rrddim_free()` with a non-zero
  refcount ("SMH_DIAG" lines).
- `src/daemon/service.c`: count why obsolete dimensions get skipped by the
  cleanup pass — too young / dictionary references > 1 / missing
  RRDDIM_FLAG_OBSOLETE ("SVC_DIAG" lines).

Return addresses were resolved with `addr2line -e build/netdata -f -C -i`
after computing the PIE load bias from `/proc/<pid>/maps` against the ELF
program headers (`readelf -l`; the bias is the runtime address of the first
`r--p` mapping, whose `p_vaddr` is 0 — naive "runtime minus text-segment
start plus file offset" arithmetic resolves to wrong-but-plausible symbols,
which cost us one false lead).

Note when reproducing: netdata re-points stderr to the `[logs] daemon`
destination, so `fprintf(stderr, ...)` diagnostics land in `daemon.log`, not
in the shell's stdout redirect.

## 3. What the Cloud AI report got right and wrong

Right, and load-bearing for this investigation: the linear growth rates, the
sawtooth-with-restart-reset shape, the flat cardinality counts, the child
`rrd`-dominant / parent metadata-dominant split, and the -738 vs -858 version
bracket.

Wrong, and worth flagging to whoever read it: its headline "confirmed leak in
dbengine and metadata subsystems on child nodes" is contradicted by its own
screenshots (child `dbengine` = 0 B; children run `mode=ram`). The 8–12 GB
"child" dbengine/metadata table values are the *parent's* profile transposed
onto children. Its "high cardinality ruled out" reasoning ("counts are flat")
is also insufficient: flat counts with growing memory is exactly what
*churn + non-freed charts* looks like, because deleted/hidden charts leave no
count behind. And "all leaking nodes run -858, correlation unambiguous" is
circular given daily redeploys.

## 4. Defect 1 — charts obsoleted at chart level are never freed

**This is the primary production leak.** Introduced 2025-04-24 by
`e46080f64f` ("Fix memory leaks and service thread corruption", PR #20159).

Mechanism, walking the current code:

- When a collector obsoletes a whole chart it calls
  `rrdset_is_obsolete___safe_from_collector_thread()`
  (`src/database/rrdset.c:116`), which sets `RRDSET_FLAG_OBSOLETE` on the
  chart. It does **not** set `RRDDIM_FLAG_OBSOLETE` on the dimensions. Via
  pluginsd, dimension flags are set only when a DIMENSION line itself carries
  the `obsolete` option (`src/plugins.d/pluginsd_parser.c:542`). The only
  in-tree setter of the dim flag is
  `rrddim_is_obsolete___safe_from_collector_thread`
  (`src/database/rrddim.c:617`).
- The cleanup pass (`svc_rrdhost_cleanup_charts_marked_obsolete`,
  `src/daemon/service.c:92`) handles a chart with `RRDSET_FLAG_OBSOLETE` by
  calling `svc_rrdset_archive_obsolete_dimensions(st, all_dimensions=true)`
  and then frees the chart **only if** that call managed to archive every
  candidate dimension (it checks that `RRDSET_FLAG_OBSOLETE_DIMENSIONS` was
  not re-armed).
- `svc_rrdset_archive_obsolete_dimensions` (`src/daemon/service.c:40`) counts
  every dimension as a candidate (because `all_dimensions=true`) and calls
  `svc_rrddim_obsolete_to_archive(rd)` for each — but that function
  (`src/daemon/service.c:5`) **early-returns false unless the dimension has
  `RRDDIM_FLAG_OBSOLETE`**. For a chart-level obsoletion no dimension has it,
  so `dim_archives < dim_candidates`, the function re-arms
  `RRDSET_FLAG_OBSOLETE_DIMENSIONS`, the caller skips `rrdset_free()`, and
  the host flag re-arms for the next pass. This repeats forever. The chart
  and all its dimensions — including, in `mode=ram`, the 28.8 KB/dimension
  ring buffers — are retained until process exit.

Why this is a *regression* and not "always broken": before `e46080f64f`, the
caller incremented `dim_archives` **unconditionally** after calling the (then
void) `svc_rrddim_obsolete_to_archive()`. The per-dimension flag check
already existed and skipped the actual archiving, but the miscount made the
caller believe everything was archived, so it proceeded to `rrdset_free()` —
which frees all dimensions regardless of flags — and memory was reclaimed.
`e46080f64f` made the counting honest (`if(svc_rrddim_obsolete_to_archive(rd))
dim_archives++;`) without making chart-level obsoletion actually archivable.
A commit titled "fix memory leaks" thereby created this leak.

Blast radius: every chart-level obsoleter. That includes cgroups.plugin when
a cgroup disappears (`src/collectors/cgroups.plugin/cgroup-discovery.c:47-83`
— container/pod churn), go.d when a job stops, pluginsd when a plugin exits
("mark all charts of exited plugins as obsolete" — added by the same
`e46080f64f`), and parents when a child disconnects without reconnecting
("mark all charts obsolete if a child does not reconnect" — same commit).

Evidence from the rig: with chart-level churn (25 charts/min obsoleted),
**all four builds** (-696, -808, -813, HEAD-883) grew `rrd` accounting at
+1.4 MB/min and VmData at +1.65 MB/min — exactly the ring-creation rate —
with `free chart` worker events at **zero**, indefinitely, while visible
chart counts stayed flat (obsolete charts are hidden from the API). The
-696 build also accumulated rrdset dictionary items (7364 items vs ~3100
visible charts after ~30 min, from `netdata.dictionaries.rrdset.items`).
Switching the churn plugin to dimension-level obsoletion (`OBSOLETE_DIMS=1`)
made `free chart`/`free dimension` events appear immediately at the churn
rate on HEAD, and the instrumented skip counters read
`skip_refs=0 skip_dimflag=0` — proving the *only* blocker for chart-level
obsoletion is the missing per-dimension flag.

Note that the fact that -696 (2026-07-05) leaks means this defect predates
the staging regression window. It is the *amplifier*, not the *trigger* —
see section 6.

## 5. Defect 2 — ram-engine metric handles are born with refcount 2

**A second, independent leak, introduced 2026-07-07 by `9ddc54a5ac`
("Netdata fixes part 35", PR #22975)** — the commit that transferred
ownership of the ram-mode ring buffer from the `RRDDIM` to the
`mem_metric_handle` so live queries can safely outlive dimension deletion.

Mechanism, in `rrddim_metric_get_or_create()`
(`src/database/ram/rrddim_mem.c`, around line 198):

    while((mh = rrddim_metric_get_by_id(si, rd->uuid)) == NULL) {
        // wrlock; JudyLIns; if slot empty:
        //     mh = callocz(...); mh->refcount = 1; indexed = true; ...
        // else refcount_acquire(...)
        // unlock
    }

After the creation branch stores the new handle (refcount = 1), control
returns to the `while` condition, which calls `rrddim_metric_get_by_id()`
again — and that function **acquires a reference** on the handle it finds
(refcount 1 → 2). The `mh` assigned inside the body is discarded; the caller
receives one handle with two references. When the dimension is later freed,
`rrddim_metric_release_from_rrddim()` (`rrddim_mem.c:298`) drops one
reference, leaving refcount 1 with no owner — the handle, and the ring
buffer whose ownership was transferred to it, live until process exit.
`pulse_db_rrd_memory` is decremented only in `rrddim_metric_free_data()`
(`rrddim_mem.c:173`), which is never reached, so the `rrd` accounting stays
inflated too — the exact counter the Cloud report shows growing.

Evidence: with dimension-level churn on HEAD (so defect 1 is bypassed and
frees actually run), memory still grew at the full churn rate. The
instrumented build then logged, for **every** freed dimension, `SMH_DIAG:
handle survived rrddim_free with refcount 1`, and the acquire-site table
showed one unbalanced site with acquires equal to the number of dimensions
ever created (14,592 at the time of capture) and zero releases — resolving
(after correct PIE bias computation) to `rrddim_metric_get_or_create` itself,
i.e. the internal `get_by_id` call. The query path
(`query_metric_add` at `src/database/contexts/query_target.c:285` paired with
`query_target_release` at `query_target.c:253`) was balanced, 742/742.

Scope: `mode=ram` (and `alloc`) hosts — i.e. exactly the staging children.
The dbengine has its own separate MRG implementation and is not affected by
this particular bug. Present in every nightly since ~2026-07-08, including
-738 ("good" only because its control node has no churn) and -858. In
staging today this defect is *masked* by defect 1 (dimensions never reach
`rrddim_free` in the churn path), but it guarantees that fixing defect 1
alone will NOT stop the leak on ram-mode children — dimensions will then be
freed, and their rings will still be retained. **Both fixes are required.**

## 6. Defect 3 — the trigger: cgroup-name helper replacement (Jul 13)

Both leaks need a churn *source* to matter, and staging had suppressed its
main one. CronJob pods (a new pod per minute per cronjob, dead seconds
later) are the pathological churn case, and the staging child config
excludes them **by resolved cgroup name**: `!*cronjob* *`. Raw k8s cgroup
paths (`kubepods-burstable-pod<uid>.slice/cri-containerd-<id>.scope`)
contain no "cronjob" substring; the exclusion works only while cgroup
renaming works.

On 2026-07-13, `70c9e64076` ("cgroups: add cgroup-name Go helper", PR
#22685) deleted the 741-line `cgroup-name.sh` and replaced it with a
compiled Go helper (`src/collectors/cgroups.plugin/cgroup-name/`), spawned
per newly-discovered cgroup by `discovery_rename_cgroup()`
(`src/collectors/cgroups.plugin/cgroup-discovery.c:196`, k8s cgroups retried
up to 9 times, `cgroup-discovery.c:1008`). It already needed a production
hotfix three days later (`ef3949dce3`, "fix cgroup-name timeout environment
race", 2026-07-16, PR #23156). This lands squarely in the -738→-858 window.

New failure modes relative to the shell script, verified locally or by
reading `cgroup-name/FLOW.md` and `kubernetes_sources.go`:

- If the binary is missing or not executable, the plugin logs
  `CGROUP: cgroup-name helper '...' is not executable; cgroup renaming is
  disabled.` and **disables renaming globally**. (Hit in the local rig with
  a build profile that skips Go artifacts; the official nightly docker image
  does ship the binary — checked `netdata/netdata:latest` on 2026-07-24.)
- On the default metadata path — the Kubernetes API server; the helm chart's
  `child.podsMetadata.useKubelet` defaults to `false` and staging does not
  override it — the Go helper **verifies TLS against the mounted
  service-account CA and fails closed**, where the shell used
  `curl -sSk` (never verified). RBAC (`pods` get/list) and node scoping
  (`?fieldSelector=spec.nodeName==$MY_NODE_NAME`, env set by the chart)
  are unchanged from the shell.

Locally, with renaming broken, the HEAD child monitored *more* cgroups than
the -696 control (raw-id `docker-<hash>-scope` names plus sub-cgroups like
`.../init` and `.../buildkit` that the name-dependent defaults previously
filtered out); with the helper present and working, HEAD and -696 monitored
an identical, correctly-named set. On a k8s node the same failure mode means
the cronjob exclusion silently matches nothing, every cronjob pod gets ~25
charts, and every pod exit is a chart-level obsoletion feeding defect 1 at a
rate that matches the observed 65–191 MiB/h.

**Status: this trigger is a strong hypothesis, not yet confirmed in
staging.** It fits the window, the mechanism, and the local behavior, but
the confirmation requires one of: (a) `kubectl logs <child pod> | grep -Ei
"cgroup-name|renaming"` on an OOMing child; (b) checking whether an OOMing
child's cgroup charts show `k8s_<ns>_<pod>_...` names or raw `kubepods...`
ids; (c) the first-OOM date (see section 8) landing on the -77x nightlies
(Jul 13-14). If renaming turns out to be healthy in staging, alternative
churn sources to investigate: go.d job lifecycle (note `a1caee5999`,
"refactor(go.d/jobmgr): replace distributed lifecycle with single-owner
command kernel", 2026-07-23 — too late for "last week" but relevant for
"this week"), increased cronjob activity, or k8s service-discovery job churn.
The leak defects stand regardless of which churn source is active.

## 7. The parent-side manifestation

The parent receives the children's charts via streaming and obsoletes the
mirrored charts chart-level (both when a child obsoletes a chart and when
a child disconnects without reconnecting — the latter path added by
`e46080f64f` too). Defect 1 therefore applies on the parent for every
churned child chart, multiplied by the fleet. In the local rig the parent —
`mode=dbengine`, receiving four churny children — grew from ~290 MB to
~710 MB RSS in ~1.5 h (~280 MB/h) with its own chart count flat; the growth
shows up in the dbengine/metadata/labels categories rather than `rrd`
(dbengine has no ram rings; the retained objects are rrdset/rrddim structs,
labels, strings, metadata). This matches the staging parent's
metadata-heavy profile and its slower ~20–21 h OOM cycle against a much
larger memory limit.

## 8. Timeline and the one open dating question

    2025-04-24  e46080f64f   defect 1 born (chart-level obsoletion never frees)
    2026-07-05  fa8bc7161b   local control build -696 (has defect 1; predates defect 2)
    2026-07-07  9ddc54a5ac   defect 2 born (ram handle refcount 2)
    2026-07-12  97dcee869d   nightly v2.10.0-738 built ("good" control node runs this)
    2026-07-13  70c9e64076   cgroup-name.sh replaced by Go helper (suspected trigger)
    2026-07-16  ef3949dce3   cgroup-name timeout race hotfix
    2026-07-23  7205ce81ae   nightly v2.10.0-858 built (staging runs this while OOMing)
    2026-07-24  92c1988f95   worktree HEAD used for this investigation (v2.10.0-883)

Open question: **the date of the first OOM kill in staging.** Because the
fleet redeploys `:latest` daily, the first-OOM date identifies the first bad
nightly directly. A ready-made prompt for Netdata Cloud "AI Insights" that
asks for exactly this (21-day per-day tables of `mem.oom_kill`, uptime
resets, build versions, per-day growth slopes, cardinality trend — facts
only, no interpretation) was prepared during the session; ask the user for
it or re-derive: the key is `mem.oom_kill` per node per day for July 3–24
cross-referenced with the build version deployed each morning.

## 9. Reproduction recipe (current rig, and how to rebuild it)

The rig from this session may still be running; whether it is or not, here
is the full recipe.

Binaries: build any tree via the netdata-build MCP (`netdata_build_start`,
profile `optimized`) or plain cmake/ninja; the per-version control builds
live under `~/opt/netdata-builds/<worktree>/netdata/usr/sbin/netdata`.

Parent config (dbengine, streaming receiver, most collectors off): bind to
127.0.0.1, port 28999; `stream.conf` with the api key section enabled:

    [11111111-2222-3333-4444-555555555555]
        enabled = yes
        default memory mode = dbengine
        health enabled by default = no

Child config mirrors staging plus the accelerated cleanup:

    [db]
        mode = ram
        update every = 1
        retention = 7200
        cleanup obsolete charts after = 60
    [ml]
        enabled = no
    [pulse]
        extended = yes        # exposes the workers job counters
    [directories]
        plugins = <install>/usr/libexec/netdata/plugins.d <dir with synthetic.plugin and churn.plugin>
    [plugins]
        timex = no
        idlejitter = no
        apps = no
        go.d = no
        charts.d = no
        python.d = no
        statsd = no

Child `stream.conf`: `[stream] enabled = yes, destination = 127.0.0.1:28999,
api key = 11111111-2222-3333-4444-555555555555`.

Launch each instance with a unique short `NETDATA_RUN_DIR` (e.g.
`/tmp/nd-oom/<name>`; long paths break unix-socket creation with
`errno 36, File name too long`) and the plugin env
(`SYNTH_CHARTS=2000 SYNTH_DIMS=5`, plus `OBSOLETE_DIMS=0|1` for the churn
mode):

    NETDATA_RUN_DIR=/tmp/nd-oom/child SYNTH_CHARTS=2000 SYNTH_DIMS=5 OBSOLETE_DIMS=0 \
        <netdata binary> -D -p 29001 -c <child>/etc/netdata.conf

Verdict signals (10–15 minutes per experiment):

- Leak present: `netdata.memory` `rrd` dimension climbs ~1.4 MB/min at the
  default churn rate; `VmData` in `/proc/<pid>/status` climbs ~1.65 MB/min;
  `netdata.workers_jobs_by_type_libuv` shows `archive chart dimensions`
  ticking but `free chart` at zero.
- Leak fixed: `free chart` ticks at 25/min (the churn rate), `rrd` and
  `VmData` plateau after the first `LIFE + cleanup` horizon (~2 min with the
  60s setting).

The two temporary diagnostic patches are committed on this investigation
branch as their own commit ("temp diagnostics", MUST be reverted before any
merge): `src/database/ram/rrddim_mem.c` (SMH_DIAG site tracking +
survival logging; also instruments `rrddim_metric_get_by_id`,
`rrddim_metric_dup`, `rrddim_metric_release`, and the survival check in
`rrddim_metric_release_from_rrddim`) and `src/daemon/service.c` (SVC_DIAG
skip-reason counters in `svc_rrdset_archive_obsolete_dimensions`). They are
marked with `TEMP DIAGNOSTIC (oom investigation)` comments. They may be kept
during fix development (the SMH_DIAG survival log doubles as a regression
check for fix 2) but must be reverted before anything merges. Remember
diagnostics print to the `[logs] daemon` destination, not the process
stdout.

## 10. Fix guidance and validation

Three independent fixes; the first two are both required to stop the child
leak, the third is required to restore the intended k8s filtering (and is
also a correctness issue on its own).

**Fix 1 (defect 1).** Two candidate shapes, both small; the design choice is
open:

- (a) Propagate obsoletion to dimensions: when
  `rrdset_is_obsolete___safe_from_collector_thread()` marks a chart
  obsolete, also set `RRDDIM_FLAG_OBSOLETE` on all its dimensions (and mirror
  the clearing in `rrdset_isnot_obsolete___safe_from_collector_thread()`,
  which matters for charts that resume collection — e.g. a chart obsoleted by
  a plugin exit and revived on plugin restart).
- (b) Keep the flags as they are and make the cleanup honest about
  chart-level obsoletion: pass the `all_dimensions` context into
  `svc_rrddim_obsolete_to_archive()` so that dimensions of a
  `RRDSET_FLAG_OBSOLETE` chart are archivable regardless of their individual
  flag. This is confined to `src/daemon/service.c`, where the counting
  regression lives, and avoids new flag-lifecycle interactions. The
  investigation lead recommends this one as the surgical option.

Beware the neighboring conditions in `svc_rrdset_archive_obsolete_dimensions`:
the `dictionary_acquired_item_references(rd_dfe.item) == 1` gate (skips
dimensions still referenced by collectors/queries — correct, they retry next
pass) and the `last_collected + rrdset_free_obsolete_time_s < now` age gate.
Neither needs changing; the skip counters proved they are not the blockers.

**Fix 2 (defect 2).** In `rrddim_metric_get_or_create()`
(`src/database/ram/rrddim_mem.c`), make the creation path hand the caller
exactly one reference. The cleanest shape is to return the newly-created
handle directly from the creation branch instead of falling back to the
`get_by_id` re-lookup (the re-lookup exists for the concurrent-creator race;
the JudyLIns result already disambiguates who created it). Alternatively
create with the understanding that the subsequent `get_by_id` adds the
caller's reference — i.e. create with refcount such that the net is 1 — but
that is subtler under concurrency. Validate against concurrent
create/delete: two racing callers must end with net refcount 2 (one each).

**Fix 3 (defect 3).** First confirm in staging (section 6). If confirmed,
the immediate mitigations are operational (fix the helper's environment /
packaging so renaming works, since the filters and k8s labels depend on it);
the code-level question — whether a TLS fail-closed policy change and a new
runtime binary dependency should be able to silently disable renaming and
thereby change what gets monitored — is a product decision that belongs to
the cgroups.plugin owners. At minimum, "renaming disabled" arguably deserves
a louder failure mode than one log line, given the blast radius.

**Validation for fixes 1+2 together:** run the section-9 rig with
chart-level churn (`OBSOLETE_DIMS=0`, the production-realistic mode) on the
fixed build next to an unfixed control. Acceptance: `free chart` events at
the churn rate; `rrd` accounting and `VmData` plateau; the SMH_DIAG survival
log (if the diagnostics are kept during development) stays silent; the
parent's memory plateaus as well (its mirrored charts are freed through the
same path). Also verify the revival path: a chart obsoleted and then
re-collected (plugin restart) must come back cleanly, and a chart obsoleted
while a query holds dimension references must be freed on a later pass once
the references drop.

**Same-failure search** (per repo policy, before closing the fix work):
other chart-level obsoletion entry points worth re-checking after fix 1 —
`svc_rrdhost_obsolete_all_charts()` (`src/daemon/service.c:168`, used on
plugin exit and child disconnect), the receiver-side obsoletion of mirrored
charts, and `proc.plugin`-style collectors that obsolete individual
dimensions (those already work — e.g. `src/collectors/proc.plugin/ipc.c:447`).

## 11. Loose ends inherited by the next session

The staging trigger confirmation and the first-OOM dating (section 6 and 8)
are open. The Cloud AI Insights onset-date prompt was handed to the user and
may already have results. The local rig's five netdata instances, three
logger scripts, and CSVs live under the previous session's scratchpad
(`/tmp/claude-1000/-home-vk-repos-nd-oom/.../scratchpad/`) and
`/tmp/nd-oom/`; they are disposable — kill the PIDs recorded in
`<scratchpad>/agents/*/netdata.pid` or simply reboot-clean `/tmp`. The
temporary diagnostics (section 9) are committed on this branch and must be
reverted before anything merges. Nothing has been pushed.

---

## Appendix A — the workload plugins

`synthetic.plugin` (constant load):

```python
#!/usr/bin/env python3
import os, sys, time, random

NCHARTS = int(os.environ.get("SYNTH_CHARTS", "2000"))
NDIMS = int(os.environ.get("SYNTH_DIMS", "5"))
out = sys.stdout

for c in range(NCHARTS):
    out.write(f"CHART synth.load_{c} '' 'Synthetic chart {c}' 'units' synth synth.load line {1000 + c} 1\n")
    for d in range(NDIMS):
        out.write(f"DIMENSION dim{d} dim{d} absolute 1 1\n")
out.flush()

begin_lines = [f"BEGIN synth.load_{c}\n" for c in range(NCHARTS)]
set_names = [f"SET dim{d} = " for d in range(NDIMS)]
rnd = random.Random(42)
while True:
    t0 = time.monotonic()
    chunks = []
    for c in range(NCHARTS):
        chunks.append(begin_lines[c])
        for d in range(NDIMS):
            chunks.append(f"{set_names[d]}{rnd.randint(0, 1000)}\n")
        chunks.append("END\n")
    out.write("".join(chunks))
    out.flush()
    time.sleep(max(0.1, 1.0 - (time.monotonic() - t0)))
```

`churn.plugin` (the leak driver; `OBSOLETE_DIMS=0` reproduces production
chart-level obsoletion, `=1` is the dimension-level control):

```python
#!/usr/bin/env python3
import os, sys, time, random

PERIOD = int(os.environ.get("CHURN_PERIOD", "60"))
NCHARTS = int(os.environ.get("CHURN_CHARTS", "25"))
NDIMS = int(os.environ.get("CHURN_DIMS", "2"))
LIFE = int(os.environ.get("CHURN_LIFE", "45"))
out = sys.stdout
rnd = random.Random(7)
gen = 0
active = []

def make_charts(g):
    for c in range(NCHARTS):
        out.write(f"CHART churn.job_{g}_{c} '' 'Churn chart gen {g} idx {c}' 'units' churn churn.job line {2000 + c} 1\n")
        for d in range(NDIMS):
            out.write(f"DIMENSION dim{d} dim{d} absolute 1 1\n")

def obsolete_charts(g):
    dim_opt = " obsolete" if os.environ.get("OBSOLETE_DIMS", "0") == "1" else ""
    for c in range(NCHARTS):
        out.write(f"CHART churn.job_{g}_{c} '' 'Churn chart gen {g} idx {c}' 'units' churn churn.job line {2000 + c} 1 obsolete\n")
        for d in range(NDIMS):
            out.write(f"DIMENSION dim{d} dim{d} absolute 1 1{dim_opt}\n")

next_spawn = time.monotonic()
while True:
    now = time.monotonic()
    if now >= next_spawn:
        make_charts(gen)
        active.append([gen, now + LIFE])
        gen += 1
        next_spawn = now + PERIOD
    still = []
    for g, expiry in active:
        if now >= expiry:
            obsolete_charts(g)
        else:
            still.append([g, expiry])
            for c in range(NCHARTS):
                out.write(f"BEGIN churn.job_{g}_{c}\n")
                for d in range(NDIMS):
                    out.write(f"SET dim{d} = {rnd.randint(0,100)}\n")
                out.write("END\n")
    active = still
    out.flush()
    time.sleep(1)
```

## Appendix B — key evidence snapshots

Chart-level churn, all four builds, identical slopes (5-minute window,
~50 obsoleted dims/min):

    HEAD: rrd +1.39 MB/min   (charts flat)
    696:  rrd +1.39 MB/min   (charts flat)
    808:  rrd +1.39 MB/min   (charts flat)
    813:  rrd +1.37 MB/min   (charts flat)

VmData, same experiment: +1664..1696 kB/min on every build.

Worker jobs with chart-level churn (2-minute window, HEAD): `cleanup
obsolete charts: 5`, `archive chart dimensions: 1975` (the same accumulated
candidates re-scanned every pass), `free chart: 0`.

Worker jobs with dimension-level churn (4-minute window, HEAD): `free
chart: 100`, `free dimension: 200` — exactly the churn rate.

Instrumented survival log with dimension-level churn (HEAD):

    SMH_DIAG: handle survived rrddim_free with refcount 1 (survival #100)
    SMH_DIAG:   site 0x...8c2 acquires=14746 releases=0     <- rrddim_metric_get_or_create (rrddim_mem.c:231)
    SMH_DIAG:   site 0x...e41 acquires=742  releases=0      <- query_metric_add (query_target.c:285)
    SMH_DIAG:   site 0x...f18 acquires=0    releases=742    <- query_target_release (query_target.c:253)

Skip-reason counters (HEAD, dimension-level churn): `archived=50
skip_young=300 skip_refs=0 skip_dimflag=0` — with dim flags set, nothing
blocks; the age gate alone defers young dims to the next pass.

Parent (dbengine, receiving 4 churny children, 51 minutes):
total accounting 291 → 644 MB (+415 MB/h), RSS → 709 MiB, own charts flat.
