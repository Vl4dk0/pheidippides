# How many tokens does phi cost?

**Measured six-message review: 3,627 tokens for agent A and 3,444 for B**
with the default `summary_every=3` (`o200k_base`, input + output). Of those,
**2,743 / 2,560 are extra versus manually relaying the same bodies**.
Loading the skill and AGENTS snippet adds **1,354 input tokens per agent**.
An idle session-start hook prints **0 tokens**.

These are text measurements, not an API invoice. The requested workflow reads
each incoming message twice: `wait`, then `read --last 1`. `wait` already includes
the full new message. Subtracting those redundant reads gives **2,219 / 2,182
tokens per agent** at the default interval, before skill loading.

## Measured items

Run on **2026-10-20**, macOS, binaries built from `6e9482d26138`.
All paired counts below are **input / output**: exact CLI stdout (plus stderr,
empty during the exchange) / agent-written shell command, body and summary.
Per-message figures are averages across all six messages, rounded to one decimal.

| One-off or session item | o200k_base | cl100k_base |
| --- | ---: | ---: |
| Load `skill/SKILL.md` (input only) | 1,279 | 1,280 |
| Load `skill/AGENTS-snippet.md` (input only) | 75 | 75 |
| Session-start `phi --as main fetch`, empty | 0 / 6 | 0 / 6 |
| Session-start `phi --as main fetch`, one unread mention | 32 / 6 | 32 / 6 |
| Optional hook output, empty (input only) | 0 | 0 |
| Optional hook output, one unread mention (input only) | 32 | 32 |

The hook executes the command without the agent generating it; its 6 command
tokens apply only when the model writes the command itself. Populated output was:

```text
phi · atlas: 1 unread threads (@alice 1) · 1 mention(s) of you (@bob/main) · 0 claimed by you
```

| Per-message/action average | summary_every | o200k_base, in / out | cl100k_base, in / out |
| --- | ---: | ---: | ---: |
| Send, including applicable summary and receipt | 3 | 55.2 / 203.3 | 57.8 / 204.7 |
| Send, including summary and receipt | 1 | 53.3 / 230.7 | 58.0 / 233.2 |
| Receive via `wait 1` | 3 | 439.7 / 8.0 | 446.0 / 8.0 |
| Receive via `wait 1` | 1 | 432.5 / 8.0 | 443.0 / 8.0 |
| Additional `read --last 1` after wait | 3 | 419.0 / 26.0 | 423.0 / 27.0 |
| Additional `read --last 1` after wait | 1 | 380.0 / 25.0 | 385.7 / 27.0 |
| `claim` or `ack`, including receipt | 3 | 3.0 / 27.0 | 3.0 / 28.0 |
| `claim` or `ack`, including receipt | 1 | 3.0 / 26.0 | 3.0 / 28.0 |

## Whole thread versus human copy-paste

A sends messages 1, 3, 5; B sends 2, 4, 6. Each side receives three messages.
Totals include one fetch per side, sends, waits, reads, B's initial claim and
A's three acknowledgements. They exclude onboarding and skill loading.

The manual baseline has each sender generate the same body once and the human
paste it into the other agent's context once. Thus each side's context contains
all six bodies, with no summaries, CLI commands, receipts or metadata. Human
copy-paste time is unpriced. The baseline is identical for both summary settings.

| Workflow | Agent | o200k_base, in / out = total | cl100k_base, in / out = total |
| --- | --- | ---: | ---: |
| Manual copy-paste | A | 459 / 425 = **884** | 459 / 428 = **887** |
| Manual copy-paste | B | 425 / 459 = **884** | 428 / 459 = **887** |
| phi, summary_every=3 | A | 2,819 / 808 = **3,627** | 2,855 / 818 = **3,673** |
| phi, summary_every=3 | B | 2,708 / 736 = **3,444** | 2,750 / 744 = **3,494** |
| phi extra over manual, interval 3 | A | 2,360 / 383 = **2,743** | 2,396 / 390 = **2,786** |
| phi extra over manual, interval 3 | B | 2,283 / 277 = **2,560** | 2,322 / 285 = **2,607** |
| phi, summary_every=1 | A | 2,555 / 860 = **3,415** | 2,615 / 879 = **3,494** |
| phi, summary_every=1 | B | 2,684 / 838 = **3,522** | 2,749 / 854 = **3,603** |
| phi extra over manual, interval 1 | A | 2,096 / 435 = **2,531** | 2,156 / 451 = **2,607** |
| phi extra over manual, interval 1 | B | 2,259 / 379 = **2,638** | 2,321 / 395 = **2,716** |

More summaries increase generated text here, but can reduce read output:
`read --last 1` also includes messages newer than the latest summary. Consequently
interval 1 has slightly fewer total tokens in this particular duplicated-read
workflow, while costing more at the illustrative prices below. Random IDs and
paths also change token counts between runs; small differences are not universal.

## Illustrative USD cost

**Prices checked 2026-10-20 — check current prices.** Standard, uncached API
text rates per million tokens; examples, not a recommendation or subscription
quota estimate. Claude/Gemini use different tokenizers: treat these as estimates
typically within roughly **±20%** for English text, not a measured accuracy bound.

| Representative model | Input / 1M | Output / 1M | Source |
| --- | ---: | ---: | --- |
| GPT-4.1 mini | $0.40 | $1.60 | [OpenAI](https://developers.openai.com/api/docs/models/gpt-4.1-mini) |
| Gemini 2.5 Flash | $0.30 | $2.50 | [Google](https://ai.google.dev/gemini-api/docs/pricing) |
| Claude Sonnet 4.6 | $3.00 | $15.00 | [Anthropic](https://platform.claude.com/docs/en/models/sonnet-4-6/overview) |

Using `o200k_base` counts, **both agents combined**:

| Model | Manual bodies | phi interval 3 | phi interval 1 | Extra over manual, 3 / 1 | Load skill + snippet on both agents |
| --- | ---: | ---: | ---: | ---: | ---: |
| GPT-4.1 mini | $0.001768 | $0.004681 | $0.004812 | $0.002913 / $0.003044 | $0.001083 |
| Gemini 2.5 Flash | $0.002475 | $0.005518 | $0.005817 | $0.003043 / $0.003342 | $0.000812 |
| Claude Sonnet 4.6 | $0.015912 | $0.039741 | $0.041187 | $0.023829 / $0.025275 | $0.008124 |

Formula: `(input_tokens × input_price + output_tokens × output_price) / 1,000,000`.
These charge each piece of text once. Real agent sessions repeatedly submit prior
context; caching, compaction, tool-call framing, reasoning, generated human-facing
commentary and the actual review work can change billing substantially. This
experiment makes no model API calls and does not measure those costs. Add skill
loading when applicable; onboarding is captured separately in the raw report.

## Reproduce

From your checkout/worktree, build its binaries explicitly into its own target:

```sh
CARGO_TARGET_DIR="$PWD/target" cargo build -p phi-cli -p phi-server
./scripts/measure-tokens > /tmp/phi-token-measurement.json
```

Requires Python 3.11+ and `uv`; the script pins `tiktoken==0.12.0` and builds
nothing. Alternatively: `uv run --with tiktoken==0.12.0 python scripts/measure-tokens`.
The script starts a throwaway loopback relay on an OS-selected free port, creates
separate temporary HOMEs for Alice and Bob, invites/joins them, and runs both
settings against separate fresh spaces. It clears inherited `PHI_*` overrides,
checks delivery and attachment bytes, and stops the relay/removes data on exit.
A competing bind can fail the run; rerun rather than selecting a fixed port.

The fixed synthetic review has a 125-word request, a 158-word first reply and
four 100–107-word follow-ups. The root attaches a 101-byte trace. Both opening
messages have summaries; later summaries are supplied only when required:
positions **1, 2, 5** for interval 3 versus **all six** for interval 1. No local
summarizer is configured. Setup/settings notifications are drained before counting.

[Raw measured report](token-measurement.json) contains exact stdout/stderr,
actual argv, agent-written commands with quoted body heredocs and inline summaries,
per-command counts, aggregate metrics, binary SHA-256 hashes and source revision.
Counts use unmodified output, summed per command; the stored text then has
temporary/home/checkout paths, the relay port and invite secrets redacted
(`<tmp>`, `<home>`, `<repo>`, `<port>`, `#<secret>`). IDs and temporary paths
vary on rerun, so expect small variation rather than byte-identical totals.

Attachment encryption/upload/download are local computation and network work:
only the filename/path/metadata printed to the agent count here. Opening the
trace would add **28 / 29 input tokens** (o200k/cl100k); charge the same content
to the manual baseline if it is pasted there too. Images have separate model
vision costs, not text-token estimates. Server operation, cryptography and GUI
rendering themselves consume **0 model tokens**; hosting and electricity are
separate costs. An external model summarizer would add its own bill.

The main levers are avoiding a second read after `wait`, using `read --last N`
instead of replaying a whole thread, keeping bodies self-contained, and choosing
`summary_every` to balance summary generation against repeated history. Idle
`fetch` prints nothing, so the optional hook adds no content tokens when idle.
