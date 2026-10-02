# AI-signs + first-use review — treight bitmap post

Reviewer: `ai-signs-review` (two lenses only: AI-writing tells per Wikipedia's field guide, and
concept-introduction ordering). Findings are advisory; the writer owns voice. No numbers are
proposed anywhere below (numbers policy respected). `{{PLACEHOLDER}}` markers are expected and are
not flagged.

- Audited snapshot: `git show 278190c2f8:posts/treight-bitmap/draft.md` (599 lines, ~5.07k words)
  — the HEAD named in the assignment.
- HEAD moved during the audit (writer commits landing live). Forward statuses below were checked
  against `6beea6d8dd` (663 lines, ~5.64k words). Line refs are pinned to `278190c2f8` unless
  prefixed `HEAD:`.
- Reference (treated as data): [Wikipedia: Signs of AI writing](https://en.wikipedia.org/wiki/Wikipedia:Signs_of_AI_writing),
  in particular its sections "High density of 'AI vocabulary' words", "Superficial analyses",
  "Promotional and advertisement-like language", "Negative parallelisms", "Rule of three",
  "Avoidance of basic copulatives", "Outline-like conclusions about challenges and future
  prospects", "Section summaries", "Overuse of em dashes", and "Ineffective indicators".

## 1. Verdict

This draft is clean at the level the Wikipedia page actually catalogs. The full "words to watch"
vocabulary sweep returns zero hits — no *delve/tapestry/landscape/testament/crucial/pivotal/
showcase/underscore/robust/seamless*-class words, no "In summary/Overall/Moreover/Additionally"
signposts, no copulative avoidance ("serves as / stands as"), no curly quotes, no
"challenges and future prospects" formula, no section-ending summary mush. The negative
parallelisms and triads that do exist all carry checkable substance, which per the assignment's
calibration rule means they are legitimate, not tells. What remains is two small genuine items
(one empty interest-signpost sentence, one repeated stance-marker) plus one density observation
(em dashes) that the reference page itself classifies as weak standing alone. Concept-introduction
discipline is unusually strong — `posting list`, `pre-order`, `canonical form` are introduced by
apposition exactly the way TLPI would — but a handful of terms in the lifecycle section are used
bare before their definition (`descriptor`, `PosSet`, `WAL`, `FST`, the tier scheme), and several
of those got *more* load-bearing in the newer commits (the results table now says "stored low+mid
tier"). Nothing here blocks anything; the first-use items are the ones worth fixing before the
numbers-fill freezes line positions.

## 2. AI-tells findings (only the empty-work hits)

Sweep method: every "words to watch" list from the reference page's Content, Language, and Style
sections, grepped case-insensitively against the pinned snapshot, then each hit (and each
near-miss shape) judged by the calibration rule: report only if no concrete, checkable substance
follows. Three items survive that filter.

### A1. Empty interest-signpost — L287 (still present at HEAD:303)

> "The interesting part is what happens to `and`/`or` when either side may be inverted."

- Tell (page vocabulary): filler meta-commentary — the nearest cataloged section is
  "Superficial analyses" (commentary attached to content instead of the content itself). The page
  has no exact name for interest-asserting frames; this is the same genus.
- Why it is empty work here: the very next sentence states the content ("Rather than
  materializing complements, the wrapper dispatches on the flag pair…"). The frame only *asserts*
  interestingness — a draft whose style notes demand "enthusiasm via precision" gains nothing by
  *saying* it.
- Suggested replacement (merge the two sentences):
  "What happens to `and`/`or` when either side may be inverted: rather than materializing
  complements, the wrapper dispatches on the flag pair and computes over the stored trees,
  De Morgan-style:" — or simply delete the first sentence and let the existing one open the
  paragraph.

### A2. Repeated fairness stance-marker — L23, L40, L100, L413 (HEAD: 39, 101, 465 remain)

> L23 "…compares against roaring honestly, including all the places roaring wins" ·
> L100 "To keep ourselves honest, treight carries a differential fuzzer…"

- Tell (page vocabulary): "Promotional and advertisement-like language" — not puffery about the
  subject, but the same self-congratulatory register applied to the post's own even-handedness.
  One use is stance; four in the first 430 lines is a tic.
- Why it is empty work here: the fairness is already *demonstrated* structurally — a "Where
  roaring wins" subsection, a limitations section, and roaring as the fuzz oracle. Announcing the
  honesty on top of showing it adds assertion, not information.
- Suggested replacement: keep exactly one instance — the section heading "treight vs roaring,
  honestly" (L413) is the natural home. L23 was already reworded away in the newer commits
  (good). For L100 (HEAD:101), drop the frame: "treight carries a differential fuzzer that uses
  roaring as the oracle ([Correctness](#correctness-differential-fuzzing-against-roaring))."
  The sentence stands fine without the preamble.

### A3. Em-dash density — observation, not a change demand

- Data: 77 spaced em dashes across 74 lines in the pinned snapshot; 84 at HEAD — roughly one per
  60–67 words, all in the spaced " — " form the reference page flags under "Overuse of em dashes".
- Calibration, honestly applied: that page section says the signal is "most useful when taken in
  combination with other indicators, not by itself" — and here the other indicators are zero
  (see verdict). The draft's usage is also varied and mostly load-bearing (appositions like
  "`treight` — **tre**e + **eight** —", punch tails like "use roaring — we do"), and the writer's
  own committed style notes are written in the same register. This is house voice, not evidence.
- Advisory only: if the writer wants to reduce the surface, the convertible instances are the
  mechanical paired asides where a comma pair or parentheses would read identically
  (e.g. L330's "a small crate — about 1,900 lines across six source files — with…"). No bulk
  replacement is suggested; that is a voice decision.

### Checked and clean (evidence the sweep ran, not findings)

- AI-vocabulary lists (all three "eras" from the page): zero hits.
- Summary-mush / "In conclusion" family; "Outline-like conclusions" formula ("Despite its… faces
  several challenges"): zero hits. §What's next enumerates concrete named series topics — no
  future-outlook mush. The one-line guidance after the Limitations bullets (L569–571) compresses
  six bullets into a decision rule rather than restating them: legitimate.
- Rule of three: every triad carries content (L539–541 names the three density bands the
  histogram actually slices; L21–23 names three post aims each owned by a section). No padding
  found.
- Negative parallelisms, judged individually and kept as legitimate because substance follows:
  L99–100 "None of this is a case against roaring. It is a case for…" (prevents a real
  misreading); L152 "You pay bytes, not pointers"; L224 "That is not a convention; it is
  asserted"; L305/321/430 "panic rather than mis-answer" (a real tradeoff statement);
  L547–548 "…because it is what we can reproduce exactly, not because it is the whole story."
- L303 "notably," introduces a genuine subtlety (short-circuit paths skip the universe assert);
  the page itself files lone transition words under "Ineffective indicators", so no action.
- L71 "state of the art" is attributed (arXiv + DOI) and immediately mechanized — substance
  follows; not puffery in the page's sense.
- Boldface usage is structural signposting, headings are sentence case, tables are real
  matrices, one thematic break before References — none of the page's Style-section shapes.

## 3. First-use audit (concept introduction ordering)

Method: for each term, the first occurrence in the pinned snapshot was located mechanically
(case-sensitivity and word boundaries enforced — the raw grep initially produced false firsts
like "WAL"→"walks", "PRIM"→"primary", "dl"→"middle") and then judged: INTRODUCED (definition or
apposition at first use), FORWARD-REF (named, with a disciplined promise of later explanation),
BARE (used before any introduction), or UNDEFINED-EVERYWHERE. Upstream status is given where the
newer commits changed the picture.

| Term | First use (pinned) | Status | Fix / upstream status |
|---|---|---|---|
| posting list | 11 | INTRODUCED | Apposition at first use ("— the bitmap of row positions where…"). Model intro. |
| universe | 46 head / 51 prose, defined 53 | INTRODUCED | Definition lands two sentences after first prose use, same breath. Optional tighten: move the apposition into L51. |
| roaring bitmap | 18 | INTRODUCED (teaser) | Formal, cited intro at 70; the L18 teaser is a deliberate hook the section pays off. Acceptable. |
| tree bytes | 20 | INTRODUCED | Derives from "one byte per node" (L19) in the same sentence. |
| range set | 13 | BARE | Used in the opening query walkthrough, never defined. **Fixed upstream** (6beea6d8dd rewords it to "the bitmap of in-window rows"). |
| blob | 168 | BARE (pinned) | `blob` ≡ serialized tree bytes was never stated in the pinned text; the reader infers it. **Fixed upstream** (gloss "the crate's name for the tree bytes" + blob→tree-bytes renames). |
| descriptor | 95 | **BARE — open (HEAD:96)** | Used in "descriptors shrink to almost nothing" ~135 lines before the definition (L230; HEAD:244). Fix: gloss at first use — "the per-bitmap descriptor (two integers — see [The Rust API](#…)) shrinks to almost nothing", or reword to "the per-bitmap handle shrinks to almost nothing". |
| node byte | 128 (figure) | INTRODUCED | Figure defines bit semantics; bullets at 139–142 complete it. |
| leaf / root-to-leaf | 121 | INTRODUCED | "root-to-leaf path *is* its base-8 representation" then the leaf-byte figure. |
| `dl` | 139 | WEAK — open (HEAD:145 vs 176) | Used in the child-index formula before its gloss "(`dl` = tree level…)" arrives at 168. Fix: move the gloss up to the 139 bullet (or add "one digit per level `dl`" at the 154–156 digit-table intro). |
| levels / depth | 118 | INTRODUCED | Explicitly equated at first use; newer commits standardize on "levels". |
| pre-order | 149 | INTRODUCED | Apposition ("a depth-first pre-order walk: emit the node byte, then…"). Model intro. |
| canonical | 215 | INTRODUCED | Apposition ("*canonical*: exactly the bytes needed, whichever path built it"). Model intro. |
| complement / `inverted` flag | 281–285 | INTRODUCED | "Pay for its holes" (281) primes "store the complement" (283). Note: "inverted *index*" (L48) is a different, standard sense — no real collision; an optional half-sentence nod ("no relation to the inverted index above") is pure voice. |
| De Morgan dispatch | 277 head / 289 | INTRODUCED | Law assumed known to the audience; the dispatch tables do the defining. |
| lockstep walker | 309 | INTRODUCED | "two readers advance in lockstep" introduces itself in-sentence. |
| `PosSet` | 249 | **BARE — open (HEAD:265)** | "(exactly what sfst's `PosSet` does)" — a named forward use with no gloss and no promise; definition arrives 145 lines later (L394; HEAD:~431). Fix: make it a disciplined forward reference — "(exactly what sfst's `PosSet`, the read-path set handle defined in [Where it lives](#where-it-lives-the-sfst-index-lifecycle), does)". |
| roaring container | 72–74 | INTRODUCED | Partition rule + member encoding stated at first use, with spec link. |
| container kinds (array/bitset/run) | 74–75; "run container" first at 425 | WEAK — open (HEAD:73 vs 477) | L74 lists the three encodings without naming them; "bitset containers" is named at 75, but "run containers"/"array container" first appear at 425/557 leaning on the reader's memory of L74. Fix: name the kinds once at L74 — "a sorted array (array container), an 8 KiB bitset (bitset container), or a run-length encoding (run container)". |
| mmap | 10 → 91 | INTRODUCED | "The file is memory-mapped" (L10) precedes the noun form. |
| WAL | 337 (diagram) | BARE — open (HEAD:~381 diagram, 400 expansion) | Acronym appears bare in the lifecycle diagram; expansion ("write-ahead log") only at 358. Fix: expand in the diagram ("WAL (write-ahead log) frames") or at first prose touch. |
| seal | 53 | WEAK — open | "one sealed batch of rows" implies but never defines the sealing event. Optional fix at L363 using the draft's own later phrase: "At seal (the WAL-to-index transition), positions must become chronological…". |
| `TimeOrder` | 346 (diagram) | INTRODUCED | Diagram glosses it inline ("insertion ⇄ chronological"); full prose at 363–365. |
| `Bitset` / bitset threshold | 352 (diagram) → 379–382 | INTRODUCED | Diagram flag + full threshold prose; the scratch-vs-stored disambiguation at 382 (vs roaring's bitset containers) is exactly right. |
| SFST / `sfst` | 3 (subtitle) → 21 | INTRODUCED | Subtitle teaser, apposition at L21 ("SFST, the indexing format behind our OTel logs and traces"); crate name `sfst` at 16/91 is consistent. |
| FST | 385 | **BARE — open (HEAD:364, 429)** | The acronym is never expanded or tied to L10's "finite-state map" ("split-FST" at HEAD:364 is equally unexpanded). Fix: at first use gloss it with the draft's own words — "the FST layer (the finite-state map from [A query, start to finish](#a-query-start-to-finish))" or equivalent. |
| `PRIM` | 339 (diagram) → 384 | WEAK (improved upstream) | Pinned used the chunk name bare in the diagram and prose; newer commits add the apposition "the primary FST chunk (`PRIM`)". Diagram-first bare use remains; an optional diagram gloss would finish it. |
| `MF{i}` | 355 (diagram) → 384 | BARE (pinned) | **Fixed upstream** — "(`MF{i}`, M for mid-cardinality)" now appears at first prose use. |
| `HF{i}` | 390 | BARE-ish (pinned) | **Fixed upstream** — "(H for high-cardinality)" gloss added. |
| tier | 386 | **WEAK — open, now more load-bearing (HEAD:431)** | The low/mid/high tier scheme is never named as a scheme; "low tier" is first *named* in the Benchmarks section (HEAD results table "stored low+mid tier"), far from the only paragraph that explains it. Fix: one sentence at Store-as-treight — fields fall into three tiers by cardinality (low, mid, high), and name which chunks each tier lands in. |
| `BitmapValue` | 355 (diagram) → 384 | INTRODUCED | The diagram's struct literal `{ desc, data }` is self-describing; prose completes it. |
| differential fuzzing | 451 (head) → 453 | INTRODUCED | The operational definition (replay on a `RoaringBitmap`, compare, roaring is the oracle) opens the section. |
| criterion | 494 | BARE — open (HEAD:548) | Micro-fix: "criterion (the Rust benchmarking harness)". |
| corpora | 54 | INTRODUCED | Self-contextualized at first use ("the corpora we benchmark against hold ~250,000 logs each"). |
| OTel | 3 | BARE — open | Never expanded to OpenTelemetry. Audience-known on a Netdata blog; optional one-word expansion at first use. |
| interner | 341 (diagram) | INTRODUCED | Diagram gloss "key=value → id". |
| facet | 346 (diagram) | BARE (micro) | Diagram jargon; optional gloss "facet (per-value counts)". |
| zstd / bincode / CRC / allocative / `RowIndex` / `ComplementIter` / `leaf_base` | 91 / 339 / 338 / 486 / 340 / 561 / 136 | INTRODUCED (domain shorthand or diagram-glossed) | Acceptable for this audience; `leaf_base` is implicitly defined by the figure plus L142 ("8 membership bits for 8 consecutive values"). |

Term-used-two-ways check: `blob` vs `tree bytes` was the one real candidate in the pinned text
(fixed upstream by the equivalence gloss); `levels` vs `depth` is explicitly equated at L118;
`Bitset` (sfst scratch) vs roaring `bitset containers` is disambiguated at L382; `inverted` vs
`inverted index` are distinct standard senses. No open double-use remains.

## 4. Re-verify plan (after the final numbers-fill)

Edits will concentrate in §Benchmarks, the results/density tables, figure captions added on the
way, and any connective prose written around the filled numbers — re-sweep exactly those, plus a
mechanical battery:

1. §Benchmarks, end to end (results table prose, Density-sliced interpretation, What this does
   not capture): number-adjacent prose is where comparative puffery creeps in ("significant",
   "substantial", "clearly faster"). Test each new comparative: does a mechanism or a table cell
   back it? That is the one AI-tell class this draft has so far avoided entirely — cheap to keep.
2. The six figure alt-texts/captions and any new transition sentences around them (all fresh
   text added after this audit's snapshot; the ones visible in the diff are factual and clean,
   but they were not part of the pinned read).
3. §What's next: currently clean (concrete named topics, no future-outlook formula). One re-read
   after any edit.
4. Re-run the mechanical battery (one grep, seconds): words-to-watch list, signpost list,
   negative-parallelism shapes, copulative-avoidance list, curly quotes, em-dash count. Baseline
   to compare against: zero / zero / the substantive set listed in §2 / zero / zero / 84.
5. Re-check the open first-use rows whose lines sit in edited regions: descriptor (HEAD:96),
   `PosSet` (265), tier scheme (431), criterion (548), run-container naming (73/477), FST
   (364/429), `dl` gloss placement (145/176), WAL diagram (~381), OTel (3).
6. Confirm no `{{PLACEHOLDER}}` remains unfilled in the shipped version — expected during
   drafting, but an unfilled one in the final reads exactly like the template artifact class the
   reference page describes (not flagged now, per instructions).

## 5. Re-verify after the numbers-fill (run at frozen HEAD `991caafd5e`)

State checked: 687 lines, 5,902 words, **zero `{{PLACEHOLDER}}` markers** (confirmed). All line
refs in this section are HEAD-relative. The two lead-flagged changes (de-self-referential sweep,
transducers link) are covered in 5.3 and 5.5.

### 5.1 Mechanical battery vs the §4 baseline

- AI-vocabulary words-to-watch: zero hits (baseline zero). No comparative-puffery vocabulary
  entered with the numbers — no "significant(ly)/substantial/dramatic" anywhere; the only
  "significant" in the file is "most significant digit first" (base-8 phrasing).
- Signposts: the single legitimate "Notably," at 317 (flags a real assert-asymmetry subtlety).
  "The interesting part is…" is gone — A1 resolved.
- Copulative avoidance, curly quotes: zero / zero (baseline zero / zero).
- `honest*`: TOC mirror (39) + section heading (467) only — exactly the recommended end state;
  "To keep ourselves honest" was removed. A2 resolved.
- Negative parallelisms: the substantive set, plus two new members from the trim/fill passes,
  both legitimate — 488 "tracks the bytes preceding the path, not just depth" (mechanism + source
  cite) and 548 "compared as relative orderings rather than sub-percent deltas" (methodology).
- Em dashes: 100 spaced (was 84; the new bench prose added some). Same register, still house
  voice, still rated weak-standing-alone per the reference page. No action.

### 5.2 Open first-use rows — resolution status

| Row (was open) | Status at HEAD |
|---|---|
| `descriptor` bare | **RESOLVED** — 96–97: "the descriptor — the two-integer handle [The Rust API](#…) defines — shrinks…" (apposition + anchor at first use) |
| `PosSet` bare forward | **RESOLVED** — 263–264: "sfst's `PosSet` — the read-path set handle, [Where it lives](#…) — does" (disciplined forward reference) |
| tier scheme unnamed | **RESOLVED** — 433–435: tiers named as a scheme with boundaries ("low under `T`, mid in `[T, 10·T)`, high at or above") before the results table leans on them |
| FST unexpanded | **RESOLVED** — 428: "`PRIM` — the finite-state map the opening query decoded"; transducers link at 431–432 is a concrete attributed pointer, no puffery, ref [5] present |
| run container unnamed | **RESOLVED** — 73: "a run container — run-length encoding" names the kind at list time |
| OTel unexpanded | **RESOLVED** — 3: "OpenTelemetry (OTel)" |
| criterion bare | **RESOLVED by context** — 547–548: used as "criterion medians" and listed under "Tools:" with its version; the two-word gloss remains optional polish |
| WAL diagram-first | **STILL OPEN (micro)** — diagram 377 "WAL frames" still precedes the expansion "write-ahead log (WAL)" at 398; fix unchanged (expand in the diagram) or accept |
| `dl` gloss placement | **STILL OPEN (micro)** — first formula use 145, gloss still at 176 |

### 5.3 Benchmarks number-adjacent prose (the §4 item 1 re-sweep)

Verdict: exemplary. Every comparative in the "Mechanisms first" paragraph and the density
interpretation is backed by table cells and carries its mechanism ("skipping exclusive subtrees
means walking their bytes; roaring compares container keys"; "De Morgan runs it over the sparse
operand's bytes"; "the union resolves without touching the other operand"; "per-container
headers dominate roaring while treight pays a byte per level"). Two advisory micros:

- "treight's whole-corpus heap is a **consistent** 0.068 of roaring's (… on jetstream)" — the
  adverb asserts a three-corpus pattern; one corpus pair is shown in-post (the README carries the
  rest). Vacuous-certainty family; wording-only fix: drop "consistent". No numbers proposed.
- "Dense lists (≥1%) favor treight more strongly the denser the list" — the shown table is
  non-monotonic in the 10–50% band on two of three corpora, and the draft's own
  complement-strictly-above-half note explains that dip. Consider scoping the "denser →
  stronger" claim to the ≥50% band. Cross-lane note: this rides table interpretation, so the
  bench reviewer should confirm intent before any rewording.
- "Mechanisms first." (two-word frame after two tables) judged legitimate: terse orientation, not
  padding.

### 5.4 Figure captions (six embeds at 152, 221, 324, 355, 368, 444)

All six alt-texts are factual descriptions of figure content; the complement and SFST-overview
captions introduce their in-figure terms by description (inverted flag, De Morgan tables,
hot/cold regions, tier routing). No tells. Clean.

### 5.5 De-self-referential sweep (referent-loss check)

No `posts/` paths, no `evidence/` mentions, and no `*.json`/`*.txt` artifact remnants survive in
prose. Every README reference resolves the same way — "the README beside this post" (553, 562,
687) — and the reproducibility sentence ("Every number here is reproducible — the README beside
this post carries the commands, fingerprints, and raw outputs…") survived the path removal
intact. No sentence in the swept sections lost its referent: the numbers the placeholders
pointed at are now inline, and artifact pointers consolidated into the README anchor. One
observation, no action: "beside this post" reads repo-relative on a published page — that is the
deliberate de-self-referential trade-off, noted for the writer only.

### 5.6 What's next

Unchanged in substance; still concrete named topics with the thesis punchline; no future-outlook
formula. Clean.

### 5.7 Re-verify verdict

**PASS** for both lenses at `991caafd5e`. All §2 findings and all load-bearing §3 rows are
resolved; residuals are four advisory micros — WAL diagram expansion, `dl` gloss placement, the
"consistent" adverb, the dense-band scoping — none blocking, writer's call on each.
