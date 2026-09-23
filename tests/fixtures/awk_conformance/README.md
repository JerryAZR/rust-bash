# BWK awk conformance fixtures

Vendored from the One True Awk (BWK) test suite
(https://github.com/onetrueawk/awk, `testdir/` — see LICENSE.bwk,
permissive Lucent terms).

- `p.*` (58 programs): from chapters 1–2 of *The AWK Programming Language*;
  run as `awk -f PROG test.countries test.countries` (two files, exercises
  FILENAME/FNR).
- `t.*` (167 programs): random constructions collected over the years; run
  as `awk -f PROG test.data`.
- `expected.toml`: ground truth recorded from **GNU Awk 5.0** (MSYS) by
  `scripts/record_bwk_conformance.sh` — stdout + exit code per program.
  Re-record only deliberately (a changed expectation is a behavior claim).

Not vendored: the self-checking `T.*` shell scripts (they embed shell
logic; ported by hand as needed), and gawk's own GPL-licensed test suite
(licensing: do not vendor GPL fixtures into this repo).
