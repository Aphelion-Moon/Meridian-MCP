# Exploration comparison: frozen before target-code inspection

2026-09-06. Exploratory, single-analyst paired study; not a blinded model trial.
Target: unchanged main Meridian-Rift checkout, revision recorded by runner.
MCP: installed memory-94c7dd9f9a5f release, isolated analysis-mode process.
Control: ripgrep file/text search and bounded, numbered source reads. No parser.
Both may refine queries. Count every attempted query/read, errors and false leads.
Prefer bounded answers (MCP search limit 3, 60 source lines); no deliberate output inflation.
At most 6 operations per arm/case; if incomplete, report what remains missing.
Shared startup/parse costs are separate, with session amortization; do not charge a
new parse per query. Repeat fixed successful/failed operation sequences for timing
only, not additional independent quality samples. No source changes or benchmarks
of compile/runtime tools. Tool schemas and setup are reported separately.

## Real questions and acceptance criteria

- R1 Exact lookup: locate /obj/machinery/vending/proc/vend; state its parameters,
  what it does to stock, and how the product reaches the buyer. Each requires source.
- R2 Inherited value: for /obj/item/storage/backpack/duffelbag, identify the effective
  w_class value, where it is assigned, and where the variable is declared.
- R3 Implementations: enumerate concrete attackby implementations on
  /obj/machinery/vending and descendants; distinguish inherited implementations
  from overrides and give locations. Verify set against source after both arms.
- R4 Concept discovery: what prevents an airlock closing on someone, and how is
  that protection disabled? Locate the decision, controlling state, and a toggle.
- R5 Cross-language: trace a Vending UI purchase from the browser action to the
  DM action handler and dispensing proc, including one rejection condition.
- R6 Added after both R3 arms returned an empty set, before inspecting spell code:
  enumerate concrete can_cast_spell implementations on /datum/action/cooldown/spell
  and descendants, with owners/locations and inherited versus overridden distinction.
  R3 remains reported as the negative lookup; R6 supplies a nonempty implementation
  case if the target exists. R6 starts with control. No score-based case substitution.
- R7 Breadth extension after the initial walkthrough, before inspecting hydroponics:
  find the periodic code that drains a hydroponics tray's water and nutrients, and
  explain what either shortage does to plant health/growth. Start MCP, then control.
  This adds a second behavior question without an exact source identifier and a
  fifth subsystem. Report its later addition and measure timing separately.

Start order alternates MCP/control for R1/R2/R3/R4/R5. First discovery in each arm
uses only the question, not the other arm's learned file/line. Follow-up source reads
use that arm's own results. Shared human memory contamination remains a limitation.

## Manufactured questions and known answers

- S1 Explicit parent: /datum/alias uses /datum/cell/special as semantic parent.
  charge is assigned 40 on special, declared on cell; reset implementation is cell.
- S2 Configuration: with ENABLE_FAST=1, /datum/configured.mode is "fast".
  Excluded orphan.dm must not contribute an implementation or value.
- S3 References: cell.reset has typed direct calls in exercise and inherited_call;
  decoy.reset, a string and a comment are not its calls. Dynamic call(receiver,...)
  is a potential runtime edge, not a proven statically resolved target.
- S4 Implementations: cell and cell/override own reset implementations; special
  and alias inherit. Excluded orphan and unrelated decoy must not appear.

Quality is scored per criterion as supported / partial / missing or wrong, with
the evidence and limitations retained. More output is not automatically better.
Metrics: operation and dependent-round counts; exact returned UTF-8 bytes/chars;
response-body versus source-text sizes; backend elapsed milliseconds; fresh parse
and reused-parse cost; Windows working/private memory. Chars/4, if reported, is
only a rough text-token proxy, never actual billed/model/reasoning token usage.

Decision rule: make MCP mandatory only if its correctness or discovery advantage
consistently pays for parsing and interaction costs across the real tasks. Otherwise
recommend selective use, preserving capabilities whose quality benefits are proven.
