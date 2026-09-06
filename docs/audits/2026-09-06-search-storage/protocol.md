# Search storage comparison protocol

Baseline: production code committed as `7755d81`, retained Windows release SHA-256 `9f17ed47707caa8ec9649f057fc38ed27f272fd84e8b986fbb98fbf43dcfc0ee`. Use clean Meridian-Rift commit `7462a6942b2e71a3ea13c00169f65f575cb281b7`. Both binaries must run against the same checkout and startup policy.

First run the ignored `retained_search_storage_profile` test with `MERIDIAN_SCALE_DME` set to the DME. It counts text occurrences, distinct allocation payloads, unique text and vector capacities. These structural counts exclude allocator overhead, hash buckets and the parser AST; they are not process-memory measurements. It only reads the environment.

For production cost and equivalence, run three fresh processes per binary, alternating binary order, with five repetitions of each query. Stop other builds/tests before measurement. Record cold-parse stages, parsed private bytes and working set, warm-reuse validation, response characters/bytes and query timings. Keep complete raw replies under ignored `target/`; publish portable samples and content hashes.

Exercise eight procedures across vending, airlocks, hydroponics, spells, storage, atmospherics and human initialization. For each, request exact inspection with default, omitted, one-line and 200-line source, plus exact-symbol search with default, omitted and 200-line source. Add varied natural-language, partial-symbol, common-term and filtered searches. The candidate must preserve complete response bodies across binaries and repetitions, excluding only build identity. Counts alone do not establish equivalence: scores, ordering, ownership, documentation, source text, truncation and snapshot metadata must all match.

Require identical parse counts, upstream/local-patch identity and source generation. Warm reuse must preserve generation and report `reused: true`. Every owned process must exit naturally with code zero. Check source Git state before and after the experiment, and verify the previously inspected source-file hashes from the source-excerpt experiment before and after measurement. Preserve any failed attempt separately.

Characters and bytes are response-volume measures, not billed or reasoning tokens. Three process samples do not establish a universal speedup. This experiment does not qualify hosted CI, BYOND runtime behavior or installation into connected tasks.
