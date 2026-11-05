# Coreutils reuse and GNU Make planning

## Decision

Use `std::fs` only for the accelerator's explicitly recognized, narrow filesystem
operations. Do **not** add uutils as a dependency in this PoC. Unsupported flags,
filesystem objects, shell constructs, or semantics must remain external commands
(or produce an explicit error), not silently approximate GNU behavior. This is a
scope/embedding decision, not a claim that uutils is unsuitable in general.

Examples of semantics that a native handler must not accidentally flatten are
`cp` symlink handling, same-file checks, preservation and directory destinations;
`mv` cross-filesystem moves and replacement; `rm` recursion and missing operands;
`mkdir` modes/umask; and `touch` existing-file timestamps, no-create and references.
A `std::fs` call is not a blanket GNU-compatible implementation. There is no claim
that the accelerator's Bash execution path is supported on native Windows.

## Reusable upstream APIs

uutils publishes individual utility crates, so linking the entire multicall
`coreutils` executable is unnecessary. Current public Rust documentation exposes:

| Crate | CLI-style in-process entry point | Lower-level entry point |
| --- | --- | --- |
| [`uu_cp`](https://docs.rs/uu_cp/latest/uu_cp/) | `uumain(args: impl uucore::Args) -> i32` | `copy(&[PathBuf], &Path, &Options)` |
| [`uu_mkdir`](https://docs.rs/uu_mkdir/latest/uu_mkdir/) | same | `mkdir(&Path, &Config)` |
| [`uu_touch`](https://docs.rs/uu_touch/latest/uu_touch/) | same | `touch(&[InputFile], &Options)` |
| [`uu_rm`](https://docs.rs/uu_rm/latest/uu_rm/) | same | `remove(&[&OsStr], &Options) -> bool` |
| [`uu_mv`](https://docs.rs/uu_mv/latest/uu_mv/) | same | `mv(&[OsString], &Options)` |

These are real reusable libraries, not merely binaries. `uumain` accepts arguments
including the executable/utility name and returns an exit status; invoking it need
not spawn a process. Lower-level APIs avoid repeated CLI parsing, but still require
constructing the utility-specific option types and auditing their side effects.
The links above are mutable `latest` documentation (observed as version 0.12.0);
the source links below refer to upstream `main`, not a pinned dependency proposal.

## Why not call `uumain` concurrently?

The upstream [entry-point macro source](https://github.com/uutils/coreutils/blob/main/src/uucore_procs/src/lib.rs)
generates a returning wrapper, but also restores SIGPIPE behavior and, where
enabled, disables Rust's SIGSEGV/SIGBUS handlers on Unix. Those are process-wide
signal dispositions, not isolated per-job behavior. Its error path prints directly
to standard error and consults the shared exit code.

[uucore's error module](https://github.com/uutils/coreutils/blob/main/src/uucore/src/lib/mods/error.rs)
contains a process-global `AtomicI32` exit code and writes diagnostics to the real
stderr. Atomic access avoids a memory race, but does not provide independent exit
statuses for simultaneous utility invocations. The [core library](https://github.com/uutils/coreutils/blob/main/src/uucore/src/lib/lib.rs)
also lazily derives utility identity and the execution phrase from process argv.
The [localization implementation](https://github.com/uutils/coreutils/blob/main/src/uucore/src/lib/mods/locale.rs)
uses thread-local localizers plus process-wide resource caches; it is not an
injected per-job diagnostic interface. These observations do not assert that every
lower-level API mutates every global; they rule out assuming that the CLI wrapper
is a pure reentrant filesystem function.

There is also no per-invocation cwd argument in `uumain`. Changing the process cwd
or environment around a call would affect other worker threads. Explicit paths
and native error returns are a simpler fit for this scheduler. Merely placing a
mutex around `uumain` would neither isolate signals/stdio from the host nor keep
parallel execution. Lower-level APIs could be revisited individually with pinned
versions and dedicated semantic tests if the supported native subset grows.

## Portability, compatibility, license and footprint

* **Windows:** the [upstream README](https://github.com/uutils/coreutils#readme)
  explicitly targets Windows as well as Linux, macOS, BSD and WASI, and documents
  a Windows feature set. The [`uu_cp` manifest](https://github.com/uutils/coreutils/blob/main/src/uu/cp/Cargo.toml)
  includes Windows-specific `windows-sys` dependencies. This supports a portability
  claim, not identical Unix permission, symlink or shell semantics everywhere.
* **GNU compatibility:** the same README says GNU compatibility is the goal and
  differences are bugs, while explicitly warning that some options may be missing
  or behavior may differ. Replacing an installed GNU binary with a library call
  is therefore not evidence of exact compatibility for every recorded command.
* **License:** the [upstream MIT license](https://github.com/uutils/coreutils/blob/main/LICENSE)
  permits reuse, modification and distribution with its copyright/permission
  notice retained, and disclaims warranty. The [workspace manifest](https://github.com/uutils/coreutils/blob/main/Cargo.toml)
  declares MIT. This is permissive reuse, not a GPL-coreutils linking requirement.
* **Footprint:** individual crates avoid the full multicall package. They are
  nevertheless not dependency-free: `uu_cp` lists Clap, Fluent, Indicatif,
  filetime, walkdir, libc, thiserror and feature-rich uucore, plus platform-specific
  dependencies. The root manifest supports selected utility features and a
  `release-small` profile. No binary-size, compile-time or throughput measurements
  were made here; a numerical overhead or performance claim would be speculative.
* **Batching practicality:** an in-process lower-level operation could avoid
  spawn/exec and repeated CLI parsing. Most listed lower-level APIs already
  accept multiple files; `mkdir` is per-path. They do not solve dependency-graph
  ordering, alias detection, failure boundaries or transactional behavior.
  Batching must retain operand order and stop/error behavior and cannot fuse
  commands merely because their executable names match. For this conservative
  subset, direct filesystem operations avoid adding the utility parser/runtime
  and make per-job cwd and error handling explicit. Broader GNU semantics should
  continue to use the original executable rather than grow an accidental clone.

## GNU Make wrapper contract

`make::plan(args, cwd)` launches `make -nB --trace -j1` with a child-specific cwd.
Stdout is read incrementally through `BufReader`, streamed to the parent's stdout
in the same byte order, and retained with Make trace lines for `read_log` target
provenance. Stderr is inherited, so there is no claimed total ordering between
the two streams. The child is waited successfully **before** returning any
`InputLog`; a nonzero/signal exit discards the partial plan. Read/stream failures
also fail planning and kill/wait the child. Non-UTF-8 output is rejected rather
than rewritten. GNU Make with `--trace` support must be available as `make`.

The wrapper uses a conservative allowlist:

* Goals, `--`, and simple named variable assignments are forwarded.
* `-f`/`--file`/`--makefile` and `-I`/`--include-dir` support separate or attached
  operands, including multiple makefiles.
* Idempotent `-n`, `-B`, `--just-print`, `--dry-run`, `--recon`, `--always-make`
  and `--trace` are allowed; `-r`/`-R`, their long equivalents, and
  `--warn-undefined-variables` are allowed.
* All other options are rejected, including abbreviated long options and
  combined short flags containing a disallowed flag. In particular `-C` and
  `--directory` cannot change the graph's cwd, `-j`/`--jobs` cannot change serial
  planning, and `-t`, `-q`, `--eval`, error-ignoring and output-control flags cannot
  alter the capture contract. The Rust CLI's worker count is separate from Make
  planner parallelism.
* Assignments overriding `MAKE`, `MAKEFLAGS`, `MFLAGS`, `GNUMAKEFLAGS`,
  `MAKEOVERRIDES`, `MAKELEVEL` or `MAKE_RESTARTS` are rejected, even after `--`.
  The child drops inherited `MAKEFLAGS`, `MFLAGS`, `GNUMAKEFLAGS`,
  `MAKEOVERRIDES`, `MAKELEVEL` and `MAKE_RESTARTS`. Parent environment is unchanged.
  Child `LC_ALL=C` makes Make diagnostics parseable; makefile expansion therefore
  runs under that locale. Stdin is closed rather than prompting or consuming the
  accelerator's input.

### A dry run is not a sandbox

GNU's [Instead of Executing Recipes manual](https://www.gnu.org/software/make/manual/html_node/Instead-of-Execution.html)
explicitly says `-n` still executes `+` recipes, lines containing `$(MAKE)` or
`${MAKE}`, and recipes needed to update included makefiles. GNU's
[`file` function documentation](https://www.gnu.org/software/make/manual/html_node/File-Function.html)
describes expansion-time file writes, and its
[`shell` function documentation](https://www.gnu.org/software/make/manual/html_node/Shell-Function.html)
describes commands executed during expansion. These can happen before the planner
fails. A failed child prevents **replaying the captured plan**, not side effects
that have already occurred inside Make.

The wrapper warns before launching and retains the warning in returned
diagnostics. Use trusted makefiles only. It does not claim that argument filtering
can prevent Makefile-authored flag mutation or arbitrary expansion-time code.
There is no reliable way to reject all forced/recursive recipes *before they run*
by examining `-n --trace` output: the dangerous execution is part of producing
that output. Silently overriding Make's shell or `MAKE` variable would instead
change the plan's semantics. Forced commands may already have run during planning
and may run again if their printed recipes are replayed; such makefiles are not a
safe replay workflow. Recursive directory changes likewise must not be flattened
into jobs running in the top-level cwd. Strict isolation would require an external
sandbox and a richer recursive-plan protocol, not a stronger promise about `-n`.

Inline `make.rs` tests use separate temporary fixture directories and real GNU
Make. They cover serial trace/target order, absent ordinary-recipe side effects,
`$(file ...)` expansion writes, a partial-output planner failure, a forced recipe's
side effect followed by failure, argument rejection, and a custom makefile with
variable forwarding. These tests passed in the integrated `cargo test` run; the
standalone CLI smoke also exercised real Make expansion and recipe replay.
