# Style notes — treight bitmap post (prose research input for the writer)

Input to the draft, not a gate. I examined well-regarded engineering posts (Lemire, Shipilëv, Dan
Luu, matklad, BurntSushi, Brooker, Cloudflare, Julia Evans) plus bitmap/indexing-specific write-ups,
and extracted the patterns that survive engineering scrutiny. Each dimension below: pattern, why it
works, exemplar link, and how to apply it to OUR section plan (outline.md). Quotes are short and
fair-use; all claims are checkable at the linked sources.

## Part 1 — Pattern catalog

### 1. Openings that earn attention without marketing fluff

**Pattern A — concrete setup, then one obvious question.** Lemire builds a plain factual runway
(compiler levels table) and then poses the question the post answers: "how much do I actually
gain?" No adjectives, no stakes-speech — the question IS the hook. See
[How much do amd64 microarchitecture levels help in Go?](https://lemire.me/blog/2026/06/06/how-much-do-amd64-microarchitecture-levels-help-in-go/).
**Pattern B — declare scope and audience in paragraph two.** BurntSushi opens with what he did and
why, then a literal "**Target audience**: Rust programmers and anyone with an interest in how one
particular finite automata regex engine is implemented." See
[Regex engine internals as a library](https://burntsushi.net/regex-internals/).
**Pattern C — open inside a real workload.** Cloudflare's best-regarded posts open at the moment
something real broke or ran; Dan Luu's study of corporate blogs praises exactly these ("Talks about a
real technical problem that impacted a lot of people, reasonably in depth") — see
[How (some) good corporate engineering blogs are written](https://danluu.com/corp-eng-blogs/) and the
[Verizon/BGP outage post](https://blog.cloudflare.com/how-verizon-and-a-bgp-optimizer-knocked-large-parts-of-the-internet-offline-today/).

**Apply:** §1 (the mmap'd two-filter query) is already Pattern C — keep it. Add Pattern B by the end
of paragraph two: one sentence naming the audience and the series position ("first in a series on
the SFST indexing machinery; this post is the smallest piece"). No "we're excited", no company
pitch. The question §1 should leave in the reader's mouth is §2's: what must a posting-list bitmap
be good at?

### 2. Structure and arc

**Pattern A — problem-first, then bottom-up tour.** BurntSushi's arc (history → "The problems" →
guided tour of engines) earns the bottom-up middle by first making the reader want it. Our outline
(§2–§3 problem/friction → §4 bottom-up build) matches. **Pattern B — table of contents for anything
over ~3k words.** Same BurntSushi post; anchor-linked, two levels max. **Pattern C — sections with a
predictable internal anatomy.** matklad's sections each open with the rule, then numbered failure
modes ([Basic Things](https://matklad.github.io/2024/03/22/basic-things.html)); this parallelism is
what makes long posts skimmable. **Pattern D — announced, bounded digressions and load-bearing
footnotes.** Julia Evans flags a digression explicitly ("we're going to take a break and talk about
CPUs for a while") in
[Fast integer sets with Roaring Bitmaps](https://jvns.ca/blog/2016/01/23/fast-integer-sets-with-roaring-bitmaps/);
Brooker parks caveats and citations in footnotes so the argument line stays clean
([Better Benchmarks Through Graphs](https://brooker.co.za/blog/2024/02/12/parameters.html)).
**Pattern E — no TL;DR box anywhere.** None of these posts has one. Long ones use a ToC plus
signpost headings; the scope paragraph does the orienting work. **Pattern F — series with named
parts and cross-links.** Julia's SQLite two-parter
([Part 1: pages!](https://jvns.ca/blog/2014/09/27/how-does-sqlite-work-part-1-pages/)) shows a
series can be forward-referenced without content-free teasers.

**Apply:** keep the outline's arc. Add a ToC after §1 (post is 3.5–4.3k words). Give §4.1–§4.7 a
recurring shape: one-sentence claim → mechanism/example → consequence. Use footnotes for the
Chambi et al. citation, the fid.c blob hash, and the fuzzer pin. Skip the TL;DR; §1 plus the §6
"where each wins" table are the anchors. Keep §10's teaser concrete (name the next posts'
subjects, as the outline already does) and link a series index if one exists.

### 3. Technical density management

**Pattern A — pause-and-convince checkpoints.** Julia hands the reader a verifiable claim and
insists: "do the math! it checks out." **Pattern B — before/after worked pairs.** BurntSushi shows
the old NFA compiler output next to the new one for the SAME regex, so the reader sees the delta
rather than trusting prose about it. **Pattern C — teach the prerequisite inline, tightly.** Before
using popcount results, Lemire explains in four sentences what a popcount is and why the baseline
can't use the instruction. **Pattern D — every complexity claim names its mechanism and worst case.**
BurntSushi states `O(m * n)` guarantees together with what enforces them (the PikeVM's lock-step
state tracking); the claim never floats free.

**Apply:** §4.3's 18-byte hand-worked example is our Pattern A moment — end it with a checkpoint the
reader can verify unaided ("an isolated value costs exactly `levels` bytes; count the chains: three
values share the leftmost path, so 6 values → 18 bytes, not 36"). Give `ceil_log8` the four-sentence
Lemire treatment (why 8^6 → 6 levels, why u32 caps at 11) the first time it appears in §4.2. For
every O() in §6, attach the mechanism in the same sentence — the outline already does this for
`contains` ("cost tracks preceding bytes, not just depth") and `len` ("walks the whole blob summing
leaf bits"); hold that bar everywhere. The §4.7 lockstep table is a Pattern B payoff: four ops the
reader has now seen separately collapse into one shape — say that explicitly.

### 4. Code and diagrams

**Pattern A — short, real, attributed snippets.** BurntSushi's code blocks link to exact source
lines (e.g. `src/lib.rs#L790-L801` in the regex repo); nothing is retyped from memory. **Pattern B —
show tool output instead of describing it.** Lemire pastes the actual `mov/movk/ret` sequence;
BurntSushi pastes `regex-cli` transcripts. Real output is unfakeable evidence. **Pattern C — real
code where the audience lives, tables/pseudo-code for dispatch logic.** matklad's
[Resilient LL Parsing Tutorial](https://matklad.github.io/2023/05/21/resilient-ll-parsing-tutorial.html)
builds from tiny real snippets to the full algorithm; BurntSushi mixes real Rust with prose
walkthroughs. **Pattern D — examples must be realistic, distilled from real code.** Julia: start from
code you actually wrote and strip it ([Write good examples by starting with real code](https://jvns.ca/blog/2021/07/08/writing-great-examples/)).

**Apply:** the two planned snippets (`from_sorted_iter` core loop from `raw.rs`, the
`intersect_subtree` match arm from `ops.rs`) are exactly the right count — attribute file paths as
the outline does, and keep each ≤ ~25 lines; resist adding a third "context" snippet. Rust is real
here (half the audience); the De Morgan dispatch and lockstep-op tables stay as tables — do not
convert them to code. Our three ASCII diagrams (tree shape, byte walk, lifecycle) are the right
medium: label every level with its `dl`, annotate leaf bytes with the values they encode (the
outline's diagram #2 annotation style), keep width ≤ 80 columns, and put the 18 raw hex bytes
adjacent to the annotated tree so the reader can align them by eye.

### 5. Numbers and evidence presentation

**Pattern A — the one-paragraph methodology disclosure.** Lemire's shape: hardware, software
versions, library pin, how many runs, how many samples — all stated before any result appears.
**Pattern B — methodology is part of the argument.** Shipilëv's benchmarking writing
([the JMH talk](https://shipilev.net/talks/devoxx-Nov2013-benchmarking.pdf),
[Nanotrusting the nanotime](https://shipilev.net/blog/2014/nanotrusting-nanotime/)) is the canon:
dead-code elimination, warmup, and measurement-infrastructure cost are addressed BEFORE numbers
appear. **Pattern C — win-and-lose framing with mechanisms.** The roaring papers themselves
([Better bitmap performance with Roaring bitmaps](https://ar5iv.labs.arxiv.org/html/1402.6407))
report where roaring loses (e.g. vs BitSet on some datasets); that candor is why the wins are
believed. **Pattern D — critique your own method out loud.** Brooker ends with what he doesn't like
about his own approach ("I don't love how the relationship between the parameters and the output
characteristics is non-linear") — one sentence, no wallowing.

**Apply:** §8 should open with the Pattern A sentence (hardware, rustc/criterion, roaring pin,
runs/samples — all from evidence artifacts; sample counts and any variance language come from the
bench files, not memory). The numbers policy already forces the right discipline: measured numbers
only via `{{PLACEHOLDER}}`, structural constants only with source attribution — keep that split
visible in §3 by labeling the per-container-overhead point as structural, not measured. Make §6's
table a genuine two-column scoreboard: every axis where roaring wins (large-operand set ops, `len`,
mutation, unbounded universe) carries the same mechanism citation as treight's wins. Add one Brooker
sentence of self-critique (e.g. what our benchmark does NOT capture). The density-slice
interpretation should tie each winning band back to the layout story, as the outline plans.

### 6. Voice

**Pattern A — flat declaratives.** Lemire's sentences carry no hedging and no amplifiers; the
numbers do the talking. **Pattern B — humility parked in footnotes, not in the argument.** Brooker:
"I've drawn them here as orthogonal, which they aren't in reality. Let's hand-wave our way past
that" — caveat registered, argument unbroken. **Pattern C — enthusiasm via precision, not
adjectives.** Julia's section headings carry the epiphany ("TIL hash sets aren't fast"); she also
models honest ignorance in-line ("I have no idea if that's good, or why") without it undermining
the piece. **Pattern D — humor dosage: dry, sparse, structural.** Brooker opens with a pun ("Isn't
the ambiguity in the word *graphs* fun?"); matklad's is a parenthetical ("Want to wrap `.md` at 80
columns? Write a test :)"); BurntSushi admits "I did some unholy hacks". One per major section, max,
and never inside an evidence section.

**Apply:** our outline mandates first-person plural; all these authors are singular, so translate
the directness, not the person — "we" means the people who built treight, never a corporate "we
recommend". Where Julia writes "I was super confused about this," we can write "the confusing part
is X" — same candor, plural-safe. Calibrate confidence exactly as the outline's honest-trade-offs
rule requires: strengths stated flatly, losses stated in the same tone, both with mechanisms.
Reserve the one or two jokes for §4 (the "tree + eight" naming and the max-value rightmost path are
natural spots); §6–§9 stay dry.

### 7. Endings

**Pattern A — the reference summary.** matklad closes with a bullet list of every point, explicitly
"for convenient reference" — a gift to the skimmer who arrived late. **Pattern B — wrap-up with
costs and future work.** BurntSushi's final sections ("Costs", "Wrap up") restate the price of the
design, not just the payoff. **Pattern C — concrete next-in-series.** Julia's posts end with
prev/next links and, when deferring a topic, say so plainly ("maybe I'll write about that another
day"). **Pattern D — reproducibility as the real call-to-action.** Lemire links the source used for
every example; Dan Luu's blog study shows the compelling posts are the honest, in-depth ones — the
CTA that doesn't cheapen the piece is "here's how to check us".

**Apply:** the outline's §9 (limitations) → §10 (series teaser) → §11 (appendix) order already
matches Patterns B/C/D. Consider a matklad-style ≤10-bullet recap just before or inside §9's end —
one line per limitation plus the one-sentence "use treight when / use roaring when" guidance from §6.
§10 stays ≤100 words, names the three next posts' subjects, promises nothing undated. End on the
README repro pointer (§11): that is the CTA. No newsletter/subscribe/contact-sales paragraph.

## Part 2 — Do / Don't checklist for the treight draft

**Opening**
- DO open inside the query (§1 as outlined); DO name audience + series position by paragraph two.
- DON'T open with company framing, "we're excited", or a problem-statement template ("In today's
  fast-paced world of observability…").

**Structure**
- DO add an anchor-linked ToC after §1; DO give §4.x subsections a repeating claim→mechanism→
  consequence shape; DO use footnotes for DOI/spec/cgit citations.
- DON'T add a TL;DR box (none of the exemplars need one; §1 + §6's table do the work); DON'T let
  §4.1–§4.7 headings exceed one line or fail to name their payload.

**Density**
- DO end §4.3 with the verify-it-yourself arithmetic (6 values, 18 bytes, three shared prefixes);
  DO introduce `ceil_log8` in ≤5 sentences before first use; DO attach a mechanism to every O().
- DON'T stack more than one new notation per paragraph (`child_index(dl, v)` and the level mapping
  deserve separate sentences); DON'T re-derive what a footnote can carry.

**Code and diagrams**
- DO keep exactly the two attributed Rust snippets (raw.rs loop, ops.rs match arm), each ≤ ~25
  lines; DO annotate the 18 hex bytes beside the tree; DO label diagram levels with `dl` values.
- DON'T invent illustrative API code that doesn't compile against the crate; DON'T convert the
  dispatch/lockstep tables into code; DON'T draw diagrams wider than ~80 chars.

**Numbers**
- DO keep every measured value a `{{PLACEHOLDER: evidence-file#metric}}` until artifacts land, then
  paste verbatim; DO disclose hardware/versions/runs/samples in §8's first sentence; DO give roaring
  its wins in §6 with the same mechanism-citation rigor as treight's.
- DON'T use relative-only claims ("2× smaller!") without the absolute numbers in the same cell;
  DON'T state a measured-sounding number in §3 (structural facts only there); DON'T editorialize
  inside tables — interpretation goes in prose after each table.

**Voice**
- DO write first-person plural with singular-level directness; DO state losses as flatly as wins;
  DO park caveats in one-line asides or footnotes.
- DON'T hedge with "might perhaps"; DON'T anthropomorphize the data structure ("treight wants…");
  DON'T joke in §6–§9.

**Endings**
- DO close §9 with the use-when guidance; DO keep §10 ≤100 words with named next subjects; DO end
  on the repro pointer.
- DON'T add a marketing CTA, a generic "more posts coming soon", or an unwritten teaser ("in a
  future post, maybe…") without a named subject.

**Compliance with the outline's binding rules**
- DO re-verify the hand-worked example's 18 bytes against the crate before final; DO keep all
  personal data out (aggregates only); DON'T resolve number placeholders from memory or estimates.

## Part 3 — Anti-patterns observed in lesser versions of this genre

1. **Corp-fluff openers.** Dan Luu's diagnosis of lame corporate blogs: "Vague, high-level fluff
   about how amazing everything is" plus handwave-y posts about the current hotness.
2. **Benchmark theater.** Tables with no hardware, no versions, no sample counts, no variance — the
   Shipilëv antipattern (dead-code elimination and warmup ignored, results measuring nothing).
3. **Victory framing.** "We replaced X" tribalism instead of per-axis win/lose analysis; readers
   who use X stop reading. Our §3's "right tool per phase" line is the antidote — keep it.
4. **De-risked vagueness.** Dan Luu again: approval processes that strip specifics make posts less
   compelling — resist any edit that removes the byte counts, file paths, or the 18-byte example.
5. **Fake examples.** Invented `foo/bar` APIs and toy inputs no one would run (Julia's point):
   our snippets and corpora provenance must be the real ones.
6. **Snippet without output.** Code blocks with no expected result rob the reader of the
   pause-and-convince checkpoint; even a one-line expected output fixes it.
7. **Unexplained wins.** "Magically faster" claims with no mechanism; every §6 cell must cite the
   winning side's structural reason or it's marketing.
8. **Explaining everything.** No digression budget, no pointers to primary sources — Julia
   explicitly declines ("I'm not going to explain how it works at all") and links roaringbitmap.org;
   we should likewise link Chambi et al./RoaringFormatSpec instead of re-deriving roaring.

---

*These notes are advisory. Where they conflict with the outline's binding numbers policy or the
Lead's decisions, those win. Strongest three recommendations, if you read nothing else: (1) make
§4.3's 18-byte walk a verify-it-yourself checkpoint; (2) keep §6 a two-sided scoreboard with
mechanism-cited cells; (3) open §8 with the one-sentence methodology disclosure before any number.*
