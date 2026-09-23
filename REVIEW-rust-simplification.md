# Rust simplification: future opportunities

## Opportunistic cleanup

- Reuse named SHA-256 helpers when code being changed repeats identical inputs
  and encoding. Preserve domain prefixes, framing and persisted fingerprint
  bytes; retain execution-inventory verification even where its aggregate is
  unused. Do not undertake a standalone abstraction pass for this cleanup.

## Further validation

These are follow-up validation opportunities, not prerequisites for the
simplification merge:

- Run sustained fuzz campaigns against the decode/import targets in
  `tools/fuzz`; investigate any failures before broadening input handling.
- Run representative performance and supported-limit workloads before making
  new throughput, memory or capacity claims.
- Run the idle deployment gate with `LIBREPAPER_TEST_IDLE_SECONDS=600` for a
  longer release-validation observation window.
