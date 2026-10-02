# Prose review — `posts/treight-bitmap/draft.md` (PASS 1)

Reviewer: prose-review (advisory input; the writer owns voice and final wording).
Date: 2026-10-02. Draft reviewed: 599 lines, 3,872 prose words (code fences, tables, and
headings excluded — inside the outline's 3,500–4,300 band). Placeholders: 31 confirmed,
treated as filled by the reader's imagination per the brief; none flagged as gaps.

Inputs: draft.md, outline.md, style-notes.md, README.md, plus a style study of TLPI
(Kerrisk, *The Linux Programming Interface*, preface + chs. 3–5, 28) via pdftotext.

---

## Verdict

The prose is closer to publication-ready than most first full drafts of this genre. The
arc is right (query → problem → credit-then-friction → bottom-up build → lifecycle →
scoreboard → evidence), the voice is consistently first-person-plural without corporate
smarm, the honesty framing survives an adversarial read (two-sided scoreboard, roaring's
wins stated as flatly as treight's, self-critique paragraph present, no sentence reads as
marketing despite the pending data), and the centerpiece — the 18-byte hand-worked
serialization — is the strongest writing in the piece; I re-derived every byte and node
of the annotated tree and it is internally correct. The reader most likely to quit is the
one who stalls in the first fifteen lines: the hook's punch sentence ("Four set
operations, zero deserialization steps") does not follow from the query just narrated,
and "a range set" arrives undefined — the two rough edges sit exactly where attention is
highest. Second quit point is §5's lifecycle diagram plus the 60-word high-cardinality
tier sentence, where an engineer skimming diagrams-first will lose the thread. Apart
from placeholders, what stands between this draft and publication is: two markdown
rendering bugs that will visibly corrupt output (unescaped pipes in the ops table;
five source paths broken mid-code-span by soft wraps), one verification paragraph whose
second derivation is mislabeled (the worst kind of bug in a "verify it yourself"
checkpoint), and a handful of claim-adjacent imprecisions ("close to that", "a real
posting list", "beats sorting a vector") that undercut the post's own brand of exactness.
Nothing here needs restructuring; every finding is a line-level repair.

## Style anchor — what TLPI models that this draft should keep doing

Studied read-only; patterns below are described in my own words.

- **Definition by apposition at first use.** TLPI introduces "a file descriptor, a
  (usually small) nonnegative integer" the moment the term appears — never earlier, never
  later. The draft does this well for *posting list* (draft L48–49); it misses for
  *universe* (used at L51, defined mid-bullet at L53) and *range set* (never).
- **Concrete example before abstraction.** TLPI shows a runnable `cp` clone before the
  per-call details: "a short demonstration" precedes "the details". The draft's §4.3
  hand-worked example is exactly this move, done well.
- **Disciplined forward references.** TLPI names the destination of every deferral
  ("Chapter 5 extends…; Chapter 13 covers…") and states the policy — "forward references
  minimized as far as possible" (Kerrisk, preface). The draft names destinations
  sometimes ("[Correctness](#…)", "the lifecycle section") and gestures other times
  ("next section but one", "benchmarks below"). Standardize on naming.
- **Terminological monotony as a feature.** One term per concept, everywhere. This is
  where the draft is weakest: blob/tree bytes, bit-tree/byte tree, three naming schemes
  for the four set ops, `len`/`len()` (details below).
- **Controlled sentence rhythm.** TLPI's explanatory prose is mostly 15–25-word
  declaratives; long sentences only carry enumerations. The draft matches this in §4.2,
  §4.3, and the roaring credit paragraph, and loses it in the hook's third sentence, the
  §5 tier sentence, and §8's methodology sentence.
- **Summaries that compress without new claims.** TLPI chapter summaries restate
  established facts only. The draft's §9 guidance line and §6 "where each wins" triptych
  obey this; keep them claim-free when numbers land.

---

## Findings — BLOCKER

**B1. draft.md:313–318 — table hygiene / rendering.** The lockstep-ops table puts raw
pipes inside code spans in table cells (`a | b`, `a & b`, `a & !b`, `a ^ b`). GFM splits
cells on unescaped pipes even inside backticks, so every row of the payoff table renders
with a spurious extra column. Suggested fix: escape inside the code spans —
`` | union | `a \| b` | copy subtree | copy subtree | `` — or switch the leaf-op column
to words (`OR of bits`, `AND of bits`, `AND NOT`, `XOR`).

**B2. draft.md:118, 282, 454, 595–597 — rendering / citations.** Five source-path code
spans are soft-wrapped mid-path (`src/crates/treight/src/` + newline + `lib.rs`, etc.).
A soft break inside a code span renders as a space, producing broken paths like
`src/crates/treight/src/ lib.rs` in the published post — every affected pointer becomes
uncopyable. Rewrap those paragraphs so each path sits on one line (or use a
non-breaking construct). Affected: L118 (`…/src/lib.rs`), L282 (`…/src/bitmap.rs`),
L454 (`…/fuzz/fuzz_targets/against_roaring.rs`), L595–596 (same fuzz path),
L596–597 (`…/sfst/src/{build,…}.rs`).

**B3. draft.md:14 — hook / claim precision.** "Four set operations, zero deserialization
steps:" — the query just narrated performs two intersections plus a count; four is not
derivable from the story. Kindest reading (four operation *kinds*) still trips the
reader at the most-attended sentence of the post. Suggested rewrite, option A (match the
story): "Two set operations and a count, zero deserialization steps: every bitmap is
intersected and counted *as stored bytes*." Option B (keep the four verbs, re-anchor):
"The operation vocabulary is four verbs — intersect, union, iterate, count — and all of
them run directly on stored bytes."

**B4. draft.md:199–200 — checkpoint correctness.** "count distinct `(level, node)` pairs
across the six paths — 1 + 2 + 3 + 3 + 4 + 5 — the same 18": those addends are nodes
*per level* (dl5:1, dl4:2, dl3:3, dl2:3, dl1:4, dl0:5), not anything a reader would get
by counting pairs across paths (that union is 6+0+1+2+4+5 in insertion order). The sums
coincide at 18, so a verifying reader who tries to reproduce the labeled decomposition
fails — in the one paragraph that exists to be verified. Suggested rewrite: "Or count
nodes per level — 1 root, 2 at dl4, 3 at dl3, 3 at dl2, 4 at dl1, 5 leaves — the same
18, and exactly what `estimate_data_size` computes."

**B5. draft.md:53–57 — claim consistency / universe.** Three sentences pull in different
directions: the universe "is the file's `record_count`" (exact), the corpora universes
are "the exact row count" (~250K each), yet the running example uses 262,144 and claims
the all-sevens path "is a real value — the very last row". If no real corpus has exactly
262,144 rows, 262,143 is not a real row; and §8 (L509) confirms per-corpus universes are
the exact row counts. Suggested rewrite: "The corpora we benchmark against hold ~250,000
logs each, so their universes land at 8⁵ < n ≤ 8⁶ — six tree levels. The running example
below rounds the universe up to 262,144 = 8⁶ so the byte-level arithmetic is exact and
the all-sevens path is the very last row of *that* universe."

**B6. draft.md:438–439 — terminology contradiction at a claims-adjacent sentence.** "six
isolated members of a 262,144-universe cost 18 bytes" directly contradicts §4.3's own
arithmetic: "An isolated value costs exactly `levels` bytes … so six values with nothing
shared would cost 36." The 18 depends on prefix *sharing* — the opposite of isolated.
The word is load-bearing in this post; it cannot mean two things. Suggested rewrite:
"the six members of the running example cost 18 bytes, not ~60 bytes of container
headers" (the ~60 is §3's structural arithmetic — keep it consistent with B8 below).

**B7. draft.md:70–71 — grammar / tonal break.** "they are everywhere we all already rely
on them" mashes two clauses ("they are everywhere" + "we all already rely on them") into
one non-sentence — in the credit paragraph, where the prose otherwise sets the tone.
Suggested rewrite: "…are the state of the art for compressed integer sets, and we all
rely on them everywhere." Or simpler: "…and they are everywhere."

## Findings — SHOULD

**S1. draft.md:21–24 — sentence length / structure.** The 50-word scope sentence stacks
three appositions inside one em-dash pair (series position, audience, promise). The
audience declaration deserves its own sentence (style-notes Pattern B). Suggested: "This
post is the first in a series on SFST, the indexing format behind our OTel logs and
traces. It is written for software engineers and Rust programmers: why the bitmap
exists, how it works down to the byte level, and where roaring beats it." This also
de-duplicates L3 (see C2).

**S2. draft.md:154 — honesty framing.** "Let's serialize a real posting list by hand" —
the values are illustrative (outline open item 3 says so); "real" is a provenance claim
the post cannot back. Suggested: drop "real" — "Let's serialize a posting list by hand."
The universe math carries the realism.

**S3. draft.md:84–87 — precision in the §3 structural accounting.** Two issues in the
overhead point. (a) "six containers × (8 + 2) bytes ≈ 60 bytes" — the container math is
exact (6 × 10 = 60); the file cookie adds on top. Write "60 bytes of container overhead,
plus the file cookie" and drop the ≈. (b) "an isolated member costs at most one byte per
tree level — 6 over this universe, less when prefixes are shared" — "less when prefixes
are shared" contradicts "isolated" (shared prefix ⇒ not isolated), and §4.3 says
isolated costs *exactly* levels. Suggested: "an isolated member costs exactly one byte
per tree level — 6 over this universe; members that share prefixes share bytes."

**S4. draft.md:87 — dangling modifier.** "and measured over thousands of real posting
lists, that difference compounds" — "measured" has no subject. Suggested: "and across
thousands of real posting lists, that difference compounds (benchmarks below)."

**S5. draft.md:279–281 — wrong hedge vs exactness.** "a naively encoded 'everything
matches' list would pay close to that" — a complete tree has every node present, so the
naive full-set encoding costs *exactly* (8⁶ − 1)/7 = 37,449 bytes. The post's brand is
exactness; hedge only where inexact. Suggested: "…would pay exactly that."

**S6. draft.md:217–220 vs 363–383 — cross-section consistency.** §4.4 claims production
use of `estimate_data_size` to conditionally convert ("if the treight size beats the
roaring serialized size, convert"), but §5's seal path decides by cardinality and density
only — no size comparison appears there. Either §5 omits a real decision or §4.4
overstates. Suggested: generalize §4.4 — "you can weigh a roaring→treight conversion
before paying for it" — or add the conditional to §5's `remap_one_bitmap` story.

**S7. draft.md:379–380 — unmeasured comparative.** "building through a scratch `Bitset`
… beats sorting a vector" is a performance claim with no evidence file behind it (the
bench measures build, but this threshold rationale is not that measurement). Suggested
reframe as design intent: "we build through a scratch `Bitset` — O(n) sets plus a
universe/64-word scan — instead of sorting a vector" and let the reason live in the
mechanism, or hedge: "which we expect to win past that density of members."

**S8. draft.md:351, 386–392 — diagram ambiguity + 60-word sentence.** (a) The diagram's
"big (≥ universe/64 or 256)?" reads as a disjunction; the threshold is
`max(universe/64, 256)`. Write "≥ max(universe/64, 256)?" in the diagram. (b) The tier
sentence nests a parenthetical citation inside an em-dash pair for ~60 words. Suggested
split: "One tier opts out of bitmaps entirely: high-cardinality fields store no
per-value bitmaps. At the default thresholds that means 1,000-plus distinct values
(`DEFAULT_CARDINALITY_THRESHOLD = 100`, high tier ≥ 10·T). Thousands of near-empty
posting lists would explode the index, so their `HF{i}` chunks are a columnar string
arena instead: … (FORMAT.md)."

**S9. draft.md:486–487 — says the opposite of the architecture.** "so both sides of
every assertion test one roaring implementation" — one side of every assertion is
treight. The point is that oracle-roaring and production-roaring are the same fork.
Suggested: "so the oracle and the production dependency are the same roaring."

**S10. draft.md:493–497 — methodology sentence.** One sentence carries hardware, three
placeholders, the pin, and the fuzzer callback; it will grow when filled. Split after
RAM: "…on a single AMD Ryzen 9 5900X with 31 GB RAM. Tools: rustc {{…}}, criterion
{{…}}, {{…}} per case with a single warm reader. The roaring side is the pinned
`netdata/roaring-rs` fork (branch `allocative`) — the same implementation the fuzzer
checks against."

**S11. draft.md:512–514 — count mismatch.** "Both phases of the lifecycle are measured"
introduces three bullets, the third of which (Size) is not a phase. Suggested: "The
bench covers both phases plus footprint:" or "Three dimensions are measured, matching
the lifecycle:".

**S12. draft.md:443–444 — word choice.** "makes storage decisions *predictive*" —
decisions are not made predictive; predictions are run before decisions. Suggested:
"turns storage decisions into cheap predictions: you can know the on-disk cost of a
conversion before paying for it."

**S13. Terminology bundle — each term one meaning (the brief's explicit sweep).**
- **blob vs tree bytes** (12 vs 8 occurrences, interchangeable): pick "tree bytes" as
  the primary (it says what they are), introduce "the blob" once as its name, or vice
  versa. Currently L20/149 "tree bytes" and L215+ "blob" alternate without a bridge.
- **bit-tree vs byte tree**: L1/19/108/591 say "bit-tree"; the §6 table (L420) says
  "8-way byte tree". Unify on "bit-tree" (the title term).
- **The four set ops have three naming schemes**: table prose
  (union/intersect/difference/symmetric difference, L313–318), code (`a.and(b)`, L291),
  and caps abbreviations "(AND/SUB/XOR)" (L323). Keep prose names in tables, code names
  in backticks, and delete the caps variant: "The pruning operators
  (intersect, difference, symmetric difference) reserve…".
- **`len` vs `len()`**: 7 bare vs 1 with parens (L113). Rust convention is `len()`;
  either is fine — pick one.
- **levels vs depth vs level count**: L118 "tree depth", L125 "Depth is derived", L210
  "level count", elsewhere "levels". Prefer "levels" (the struct field) except where
  "depth" reads better as tree geometry — but not three words for one number.

**S14. draft.md:13 — undefined jargon in the hook.** "intersecting with a range set" —
"range set" appears once, undefined, in the reader's first ten lines. Suggested: "then
clips the result to the time window with the bitmap of in-window rows".

**S15. draft.md:446 — number vs phrase.** "a handful of probes either way at this
universe size (six for treight, two for roaring)" — two is not a handful, and the
parenthetical contradicts the phrase. Suggested: "a few probes either way (six for
treight, two for roaring)".

**S16. draft.md:590–591 — reference grammar + missing pin.** "(the bit-tree treight
ports, by Greg McGary)" is missing its "that"; and the outline's verified blob hash
(8fb6ca4c) never made it in. Suggested: "GNU idutils `fid.c` — the bit-tree that
treight ports, by Greg McGary (Savannah cgit, blob 8fb6ca4c):".

## Findings — CONSIDER

**C1. draft.md:104 — §4 has no lede.** The longest section (2,200+ words) jumps
straight into 4.1. TLPI opens every chapter with where-we-are + scope + deferrals in
3–4 sentences. Suggested: two sentences after the §4 heading — "This section builds
treight from the bottom up: where the layout comes from, what a node byte is, how the
tree serializes, and what the four set operations share. Everything after that — the
lifecycle, the scoreboard, the numbers — is consequences."

**C2. draft.md:3 + 21 — series line duplicated; SFST never expanded.** L3 and L21–22
say nearly the same sentence 18 lines apart. Trim one. Separately: SFST is described
twice but never expanded — if the letters stand for something, expand at first use; if
not, say so once ("SFST, our name for the format") so readers stop looking for the
acronym.

**C3. draft.md:119–120 — subject of "The count shifts…".** "The count shifts the
largest index right by 3 bits at a time … counting the shifts" — "count" is both
subject and gerund-adjacent noun. Suggested: "`ceil_log8` shifts the largest index
right by 3 bits at a time until nothing is left, counting the shifts."

**C4. draft.md:125 vs 233–236 — "never stored" vs the `levels` field.** §4.2 says depth
"is derived from the universe, never stored"; §4.5's descriptor struct has `levels: u8`.
Both are true (not on the wire; kept in memory), but a careful reader trips. Add half a
clause at L239: "two integers — universe size and tree depth (re-derived on load,
never serialized) — and it is `Copy`".

**C5. draft.md:55, 141, 279 vs 280 — exponent style.** `8^dl` in the §4.2 bullet vs 8⁶
unicode superscripts in prose. Inside code spans `8^dl` is right; in prose pick the
superscript consistently (8⁶, 8⁵).

**C6. draft.md:63, 145 — repeated formula.** "There is no such thing as an in-place
posting-list edit" / "there is no such thing as a stored zero byte" — the same
rhetorical construction twice, 80 lines apart. Vary one ("no zero byte is ever stored"
is already half-present at L326).

**C7. draft.md:239–241 — two method shapes where there are three.** "`&[u8]` for
queries, `&mut Vec<u8>` for mutations" — set operations take both (read two slices,
append to an out buffer), as L247 immediately says. Suggested: "`&[u8]` for queries,
`&mut Vec<u8>` for builds and mutations — and set operations take two slices plus an
output buffer."

**C8. draft.md:301–305 — 45-word assert sentence.** Two ideas (short-circuit
correctness; release-mode panic) in one semicolon-and-dash chain. Split at the
semicolon: "…before any walk. Notably, the short-circuit paths return value-correct
results without checking universe equality, while every dispatched path asserts it —
the raw ops assert unconditionally, so a mismatched-universe bug panics in release
builds too."

**C9. draft.md:325 — "A two-operand `intersect`".** Intersect is inherently two-operand;
the qualifier reads like a distinction from an n-ary variant. Drop it.

**C10. draft.md:330–331 — slash variant + uncited count.** "our log/traces index format"
vs "logs and traces" elsewhere; and "about 1,900 lines" is a structural constant by
outline policy (attributable) but carries no citation — either add a `wc -l` style
pointer with the source pointers or let "about" carry it consciously.

**C11. draft.md:11 vs 342 — service=edge vs service=x.** The hook pulls `service=edge`;
the lifecycle diagram filters `service=x`. Same query, two names — unify (edge is
better: it echoes the §2 density example).

**C12. draft.md:384–392 — MF{i}/HF{i} unexplained letters.** PRIM is glossed (L10),
MF{i} half-glossed ("per-field"), HF{i} only by implication. One clause — "M for the
per-field bitmap tier, H for the high-cardinality tier" — or defer the chunk taxonomy
to the next post in the series and say so.

**C13. draft.md:430–436 — paragraph anatomy.** "Where roaring wins" mixes two labeled
points (Cardinality:, Bulk operations:) with two bare fragments ("Anything
mutation-heavy. Anything needing…"). The fragments are good voice; the mix reads like a
list that forgot its bullets. Either bullets throughout or fold the fragments into one
closing sentence.

**C14. draft.md:497 — verbed noun.** "the same implementation the fuzzer differentials
against" — "differential" is doing verb work it doesn't do in English. "checks against"
or "diffs against".

**C15. number formatting.** "~250,000 logs" (L54) vs "~250K events" (L503) — unify
(prose numbers get commas: ~250,000). Value literals inside fences/tables stay bare
(262143, 4097) — that rule is currently consistent; keep L198's "262,143" comma'd since
it's prose.

**C16. heading parallelism.** §4.x mixes colon-subtitle form (Lineage:, Canonical
form:, The Rust API:, Dense sets:) with plain noun phrases (Node bytes and the tree
shape; Pre-order serialization, by hand; One walker, four operations). All name their
payload and fit one line — acceptable as-is; if you touch them, unify toward the
colon form.

**C17. style-notes deviations to ratify or repair.** (a) The planned `ops.rs`
match-arm snippet never landed — §4.7 explains back-patching in prose only; the two
extra snippets that did land (remap_one_bitmap, check_equal) earn their lines, so this
is fine, but decide consciously. (b) Citations are an end-of-post reference list, not
the footnotes style-notes sketched — fine for the blog format; the missing fid.c blob
hash is the only real gap (S16). (c) Audience lands in ¶3, style-notes wanted ¶2 —
one sentence earlier is all it takes (S1).

**C18. draft.md:578–580 — closing slogan vs the zstd nuance.** "the file you mmap is
the structure you query" compresses past the body's own honesty (chunks are zstd-framed
and decoded once on open, L91–93). The slogan is earnable if reworded to the claim the
body actually defends: "the bytes you store are the structure you query." If the mmap
flavor must stay, the body nuance already carries it — writer's call.

**C19. draft.md:536–541 — two compressions.** "complement off" (reads like a switch)
→ "no complement needed"; "which of those hold" → "which of those expectations hold".

**C20. em-dash density.** 83 em-dashes across 74 lines. Mostly load-bearing, but the
stacking inside single sentences (L21–24, L301–305, L386–392, L493–497) is what creates
the 45–60-word monsters flagged above; fixing those four sentences also fixes the
density.

**C21. draft.md:77 — "next section but one".** Correct idiom, correct referent (§5),
but register-mixed and easy to mis-scan. Naming beats counting: "(see
[Where it lives](#where-it-lives-the-sfst-index-lifecycle))".

## Sweeps that came back clean

- **Passive voice**: rare and conventional where it appears (methodology "were taken",
  the deliberate object-fronting at L14). No finding.
- **its/it's**: zero "it's"; all possessives correct (grep-verified).
- **a/an**: correct before vowel-sounding numerals ("an 8 KiB bitset", "an 8-byte
  header", "a u32 universe").
- **Comma splices**: none; the semicolon chains (L552–554) are deliberate and read as
  such.
- **ToC anchor links**: all 18 resolve against GitHub-style slugs of the 18 headings
  (script-verified), including the backticked `Copy` heading.
- **Table standalone-ness**: every table (De Morgan ×2, lockstep ops, §6 scoreboard,
  §8 results) is interpretable without surrounding prose — except the ops table's
  rendering bug (B1), which is a syntax issue, not a content one.
- **Code fences**: five attributed snippets, each ≤ 25 lines, each earning its lines;
  elisions marked with `// …` exactly once.
- **Jargon budget**: posting list, universe, descriptor, mmap, FST all fine for both
  audiences after the S14 fix; "bincode" (L339) is the only un-glossed name — one word
  of context or leave it.
- **Honesty framing**: no sentence reads as marketing; roaring's credit paragraph and
  win list are as mechanically cited as treight's. The three soft spots (S2, S5, S7)
  are imprecision, not spin.
- **§4.3 arithmetic**: hex bytes, node bytes, offsets, leaf values, per-value cost
  accounting (5+2+2+4+5), and the wire dump all re-derived by this reviewer and
  correct — apart from the B4 label.

## PASS 2 readiness notes (for when numbers + figures land)

1. Figures: `figures/` has six targets (bitree, serialization, setops, complement,
   lifecycle, sfst-overview); bitree + serialization are populated, setops has assets
   but no DESCRIPTION, complement/lifecycle are empty as of today. The draft embeds no
   figures and has no `{{figure}}` markers — all 31 placeholders are evidence/bench.
   PASS 2 must place the embeds (replace vs augment the ASCII diagrams), and captions
   must not duplicate the sentences that introduce each diagram (those sentences
   already stand alone — good).
2. L509 "over a universe of {{rows-per-corpus}}": singular phrasing for a per-corpus
   value — check the fill's shape (three numbers? a range?) and pluralize around it.
3. §8 results table: decide once whether unit suffixes live in row labels or in cells;
   fills must be bare or suffixed consistently across all 21 cells.
4. After fills, re-check: density-slice winners vs §6's tie claims; "which of those
   expectations hold" sentence vs the actual slices; hedging around `len`/build deltas
   (§6 promises the benchmarks "quantify" the len gap — the filled text must actually
   do so).
5. Re-run the anchor script and the pipe/wrap greps after the writer's edits — both
   classes of rendering bug are one reflow away from recurring.

## Mechanical checklist (10 items)

1. **its/it's** — clean (no "it's" in the draft; all "its" possessive). ✔
2. **a/an** — clean ("an 8 KiB", "an 8-byte header", "a u32"). ✔
3. **Comma splices** — none found. ✔
4. **Hyphenation** — consistent ("self-delimiting", "machine-word-parallel",
   "zstd-framed"); "mmap-able" vs "memory-mapped" differ in role, acceptable. ✔
5. **Number formatting** — one real clash: ~250K (L503) vs ~250,000 (L54); unify (C15).
   Bare-vs-comma rule for value literals is otherwise consistent. ✗ (one fix)
6. **Units** — bytes/KiB/GB used correctly and consistently; "u32 LE" glossed. ✔
7. **Capitalization** — treight lowercase everywhere including sentence starts
   (deliberate, consistent); roaring lowercase; SFST/PRIM/MF{i}/HF{i} caps consistent;
   "De Morgan" correctly two words. ✔ (SFST expansion question open, C2)
8. **Link text** — numeric reference links + one named section link; reference list
   complete with titles; add the blob hash (S16). ✔ after S16
9. **Heading parallelism** — mixed colon/plain in §4.x (C16); all one line, all
   payload-naming. ~
10. **Em-dash/quote glyphs** — straight quotes only, no smart quotes; box-drawing
    confined to fences; 83 em-dashes (C20) with four sentences carrying stacked pairs. ~

**Top-priority order for the writer**: B1, B2 (rendering, mechanical), B3 (hook claim),
B4 (checkpoint label), B5+B6 (universe/isolated consistency), then S1–S16 in order.

---

# PASS 2 — the filled draft (numbers and figures in)

Date: 2026-10-02, late evening. Draft reviewed: 687 lines, 4,471 prose words by my
counting method (fences/tables/headings excluded; the Lead's method says 4,495 — both
under the 4,500 ceiling). Zero placeholders remain. PASS-1 findings: all seven BLOCKERs
applied correctly, plus the SHOULDs and CONSIDERs I re-checked individually. Every
number below was verified directly against the certified artifacts
(`evidence/bench/results.json`, `totals-*.md`, `environment.txt`, `per-list-*.csv.gz`,
`evidence/corpora/stats.md`).

One correction to my own PASS 1: the draft then said "64 joins the top three bytes,"
which I verified as correct — it was not. 64 = 0o100 shares root + 0x01 + 0x02 + 0x03
(four bytes) before its own 0x07/0x08. The current draft says "top four bytes," which
is right; I re-derived the full 18-byte example from scratch this pass and it is
correct as published.

## Verdict update

With results visible the post delivers what §3 and §6 promise: the footprint story
(0.51–0.65 of roaring on stored tiers) pays off "that difference compounds," the
mid-density concession is made in §6 before the reader reaches the band table, and the
pair rows are striking AND mechanistically explained. The honesty framing mostly
survives contact with the numbers — point queries were correctly reclassified from
"tie" to "roaring wins," matching 387–6,973×. Two residues break that standard: the
footprint-ratio row is not derivable from the table's own visible cells (P1), and
range counting still sits under "Where they tie" while the table shows 35–288× in
roaring's favor (P2). Both are line-level fixes. The abstract-level flow is sound;
the one place the 4,500-word ceiling shows is the bench-coverage bullet (P8), which
reads as compressed noun phrases rather than prose.

## Verified clean against the artifacts (so the writer doesn't re-check these)

- All 21 CPU-table cells and all three pair rows reproduce from `results.json`
  (recomputed ratios match to rounding: bulk build 2.48/1.71/0.89…, iteration, contains
  387–6,973×, range_count 44–234/35–264/80–288, len() 686–16,637/828–19,844/753–10,039,
  and-pairs 43/52/2.6, and_not 0.049/0.051/0.053, or-pairs 4.9/4.6/4.8 ns vs
  2,931/2,987/2,814 ns — all verbatim).
- All 42 density-table cells and all footprint cells match `totals-*.md` exactly.
- "consistent 0.068": 0.0678 / 0.0683 / 0.0680 across the three corpora — true; the
  quoted jetstream pair 17,799,801 vs 262,065,816 is verbatim.
- 32,808: verified structurally (4 bitset containers × 8,192 + 32 B headers + 8 B
  cookie) and by artifact (jetstream ≥50% roaring 328,080 / 10 lists).
- "per-list slices near 44% flip to roaring (1.07×)": exact — certstream list at
  density 0.440, 35,020 vs 32,808 bytes = 1.067.
- "9 of the ≥50% bucket's 12" exactly-half in mixed: consistent with 12 lists − 3
  complemented; per-list rows confirm density-0.500 card-125,000 lists encoded direct.
- Environment claims: rustc 1.95.0 (59807616e 2026-04-14) ✓, 50 samples / 1 s warm-up
  / 2 s measurement ✓, roaring pin branch `allocative` commit `3ee69b1` ✓, "relative
  orderings, not sub-percent deltas" mirrors environment.txt's own guidance ✓.
- Corpora: 250,000 lines each ✓, mixed 125,000 + 125,000 ✓, no kv strings quoted in
  the post (the per-list CSVs deliberately exclude them — the "aggregate only" claim
  holds).
- Figures: all six referenced SVGs exist on disk; every alt text is descriptive; every
  figure sits between standalone prose that does not duplicate the alt text.
- Mechanics: 0 placeholders, all 22 anchor links resolve, ops-table pipes correctly
  escaped (`a \| b`), no code-span wraps remain, blob/tree-bytes now bridged once
  (L176) with two licensed later uses.

## Findings — PASS 2

**P1. BLOCKER — draft.md:574–576 — the footprint-ratio row is not derivable from the
table's own cells.** The shown ratios 0.510 / 0.647 / 0.630 are the artifact's
tree-bytes-only ratios (`totals-*.md`, "serialized bytes" rows: 1,792,162/3,514,248,
994,834/1,538,624, 2,111,260/3,353,048), but the treight row above them is tree + 5 B
descriptor (1,797,922 / 1,115,004 / 2,212,840). A reader dividing the visible cells
gets 0.512 / 0.725 / 0.660 — jetstream is 12% off the shown 0.647. Fix (policy-clean,
verbatim from the artifact): split the treight row the way `totals-*.md` does —
"serialized tree bytes | 1,792,162 | 994,834 | 2,111,260" and "+ 5 B descriptor per
list | 1,797,922 | 1,115,004 | 2,212,840" — and keep the ratio row on tree bytes; or
at minimum relabel the ratio row "(tree bytes / roaring serialized, descriptors
excluded)". Writer + bench-eng should pick the shape; the current one fails
self-consistency.

**P2. BLOCKER — draft.md:503–504 — "Where they tie" now contradicts the visible
numbers.** Range counting stays under the tie heading, but the table row (L590) shows
`range_count` at 35–288× in roaring's favor. With results visible, the tie label reads
as spin — the one place in the filled draft that does. Fix: move the sentence into
"Where roaring wins" with the mechanism as caveat — "Range counting: both structures
prune (roaring sums container counts, treight skips subtrees outside the range), but
the constants differ — 35–288× measured — so it wins there too." — or keep a tie
heading only if the sentence states the measured delta.

**P3. SHOULD — draft.md:600 — "≈3–11 ns vs 5–28 µs per probe" mixes bases.** The
roaring range (3–11 ns) spans all corpora; the treight range (5–28 µs) is
mixed-corpus-only (its lt1 hit 4.95 µs to gt50 miss 28.0 µs). The full artifact
ranges: treight hits 3.9–18.8 µs, misses 4.9–39.8 µs (certstream's gt50 miss is
39.8 µs — above the stated top). Suggested: "≈3–11 ns vs ~4–19 µs on hits, up to
~40 µs on misses" — or scope the sentence to the table's basis (hits) explicitly.

**P4. SHOULD — draft.md:604–606 — the or/cross-field mechanism under-discloses the
measured pair.** The numbers and the stated condition are accurate, but in all three
measured pairs BOTH operands are density-1.0, card 250,000 (e.g. mixed's
`scope.version=0.1.3` over `severity_number=9`); "when one operand is
complement-stored full" is true of the pair but incomplete. A reader may generalize
~5 ns to arbitrary cross-field ORs. Suggested addition: "— and in the measured pairs
both operands are full everywhere-fields, so the short-circuit fires before any byte
is read." This is the post's most striking number; it deserves the extra clause.

**P5. SHOULD — insert benches are in the artifact but absent from the post.**
`results.json` carries `insert_arrival`/`insert_shuffled`: treight measures 129–690×
slower per position than roaring. The post's coverage line says "Build and query" and
the limitations section concedes mutation, but the measured insertion gap is never
cited. As silence it is pro-treight spin; as a sentence it becomes evidence for the
design: "Incremental insertion measures 129–690× roaring's per-position cost — the
number the seal path exists to avoid." Writer/Lead call under the word ceiling; the
artifact supports it verbatim (recomputable from results.json).

**P6. SHOULD — draft.md:625 — "23/10/3 lists per corpus" cross-checks badly against
the density table.** Those are the complemented-list counts (correct per totals), but
the reader who checks the ≥50% row sees 23/10/12 and finds mixed mismatched with no
explanation for the first two corpora. Suggested: "(complement encoding — 23/10/3
lists complemented per corpus; strictly above half only, so exactly-half lists stay
direct: 9 of mixed's 12 in the ≥50% band)".

**P7. SHOULD — draft.md:548 — "criterion 0.5.1" appears in no evidence artifact.**
environment.txt records rustc, allocative, and the roaring pin, but not the criterion
version. Under the post's own "every number here is reproducible" banner, get
bench-eng to add it to environment.txt, or cite `Cargo.lock` inline.

**P8. SHOULD — draft.md:564–567 — the bench-coverage bullet reads as cut-to-fit.**
"Build and query per corpus × density bucket, plus operand pairs; size as summed
bytes and allocative heap." is a verbless noun-phrase pileup — the one place the
4,500 ceiling visibly pinches. Restore three short bullets or one flowing sentence,
and gloss "allocative" ("deep-size via the allocative crate") — engineers outside the
Rust ecosystem won't know it. (Lead owns the ceiling call; the three-bullet form adds
~8 words.)

**P9. CONSIDER — draft.md:349–350 — "in four bytes per operand": B is three bytes
(`05 22 03`).** "in three or four bytes per operand" — the A/B asymmetry is part of
the point (B is smaller).

**P10. CONSIDER — draft.md:600 — "by three to four orders" — the measured ratios are
387–6,973× (2.6–3.8 orders); the floor undershoots "three orders."** Echo the table
instead: "by 387–6,973×," or "by up to nearly four orders."

**P11. CONSIDER — draft.md:579 — "though a real file stores it as arenas" — it/they
mismatch (the high tier's lists → them).** "though real files store that tier as
arenas."

**P12. CONSIDER — draft.md:547 — "31 GB RAM" vs environment.txt's "31 GiB".** Use
GiB; the post is otherwise exact about units.

**P13. CONSIDER — draft.md:677–680 — reference list order: [5] (Transducers) is
listed above [4] (idutils).** First-citation order is correct ([4] in §4.1, [5] in
§5); the list is just out of numeric order. Reorder.

**P14. CONSIDER — em-dash density rose from 83 to 110.** The §8 mechanism paragraph
(L596–606) carries five across three sentences; two could become parentheses or
colons at zero word cost.

**P15. CONSIDER — draft.md:497 — "against roughly 60 of container overhead" — §3
(L85–86) now states the exact 60; §6's "roughly" re-hedges it.** "against 60 of
container overhead" (the cookie already accounted in §3).

**P16. CONSIDER — method-paren style is mixed: `is_empty` (L234) vs `len()`
(L476+).** Pick one convention for zero-arg methods and apply it.

**P17. CONSIDER — draft.md:549 — "a single warm reader" has no backing sentence in
environment.txt** (which covers sharing and noise, not reader warmth). Ask bench-eng
for one clause there, or drop "warm" — "What this does not capture" already concedes
cold-page behavior is unmeasured.

## Cut-to-fit sweep (Lead's request)

1. L564–567 coverage bullet — the clear pinch point (P8).
2. L499 — "linear byte scans" lost its prefetcher justification from the PASS-1 text;
   acceptable loss, restore only if words free up.
3. L541–543 — "…all inverted-flag combinations included" tail reads clipped; two more
   words ("included, per flag pair") would smooth it.
4. L503–504 — the tie section shrank to one sentence while its sibling claims moved
   to roaring-wins; range_count was left behind (that is P2 — arguably a trim artifact
   rather than a judgment).

## PASS 2 bottom line

Thirty-nine of forty-two number-adjacent claims verified verbatim; the three that
fail are P1 (ratio basis), P3 (mixed bases in one sentence), and the P2 labeling
residue — all repairable in under an hour of writing. After P1, P2, and the P3–P8
SHOULDs, I have no further objections to publication on prose, number-integrity, or
honesty grounds. This review pass found no reason to reopen structure, figures, or
the §4 core.
