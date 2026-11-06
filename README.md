# rusty_make

A small Rust core for replaying command logs as typed jobs and sequential,
dependency-safe batches. It does not invoke Make or implement a general shell.

## Run

```sh
cargo build --locked
./target/debug/rusty_make --commands commands.txt --verbose
cargo test --locked
cargo run --locked --example python_generated_headers
```

The CLI has two functional options: required `--commands FILE` and optional
`--verbose` (`-v`). Commands inherit the process working directory, **not** the
log file's parent. External programs must be installed and available through the
supplied executable path or PATH. Do not replay untrusted command logs.

For example, given an existing `source` file:

```sh
mkdir -p build/nested
cp source build/nested/copied
mv build/nested/copied build/nested/moved
touch build/nested/stamp
cp build/nested/moved result
rm -rf build
```

Save these lines as `commands.txt`, then run the executable from that directory.
The native operations need no external coreutils programs or Bash.

## Pipeline

1. Read physical log lines, joining active backslash-newline continuations,
   including CRLF. Keep each command's physical starting line and original text.
2. Tokenize with Brush in-process, decode literal words, and extract shell hints.
3. Dispatch by registered executable basename into concrete jobs. A handler may
   use clap to extract useful arguments while retaining the original argv.
4. Optionally enrich compiler inputs using the header trie and `Any` downcasts.
5. Build the generic file-access DAG and remove transitive edges.
6. Check unproduced input reads before executing any job, then execute concrete-
   type batches sequentially. Stop at the first unmasked failure.

There is no cache, persistence, execution strategy, job serialization, stored
artifact-version model, general shell fallback, cwd state, path canonicalization,
copy-chain/unity rewrite, or parallel command execution. Removed CLI options such
as `--jobs`, `--dry-run`, and plan/compilation-database exports are errors.

## Accepted command syntax

- Literal unquoted, single-quoted, double-quoted, and escaped words.
- Stdout `> file` and stderr `2> file`, opened in source order. Each stream may
  have at most one redirect. `/dev/null` maps to a null stream on either host;
  Windows additionally recognizes `NUL` and `NUL:`.
- A final `|| true`, `|| :`, or `|| cd .`. These set `ignore_failure`; they do
  not run another command or change directories. Runtime/spawn/redirect failures
  are masked, but invalid syntax and rejected handler arguments remain errors.
- Blank lines, comments, and recognized non-command Make trace records.

Pipelines, arbitrary command lists, assignments, expansions, globbing, input or
append redirection, descriptor duplication, and directory-changing recursive Make
logs are rejected. There is no parsing subprocess and no fallback to Bash.
The whole log must parse and classify successfully before execution starts.

## Handlers

Registrations are explicit names, not regexes. `/usr/bin/nm` dispatches through
`nm`; a final `.exe` suffix is stripped for dispatch only. Execution retains the
full original executable spelling and argv. Cross-toolchain or versioned names
need explicit registration.

| Family | Behavior |
| --- | --- |
| `cp`, `mkdir`, `touch`, `rm`, `mv` | Narrow native filesystem operations; unsupported options are errors. See [native semantics](docs/coreutils.md). |
| `cc`, `c++`, `gcc`, `g++`, `clang`, `clang++` | Extract sources, outputs, include directories and selected compiler flags; run original argv. Uncertain effects become ordering barriers. |
| `sed`, `gsed` | Original external execution; simple substitutions and in-place/backup forms have file metadata. No sed fusion. Same input/stdout spelling is rejected; use `-i` without that redirect. |
| `gawk`, `awk` | Original external execution with conservative file metadata. |
| GNU/LLVM binary tools | Explicit aliases for symbol inspection, assembly, linking, archives and binary mutation; original argv, conservative effects. Registrations live in `src/commands/mod.rs`. |
| `coan` | Source stdout/replacement jobs; compatible Unix jobs can use copy-then-replace fusion. |
| `python`, `python3`, `py` | Nested script-specific dispatch, not a generic Python invocation parser. The built-in handler recognizes `generate_header.py`. |
| Explicit external utilities | `register_external` creates unknown-effect jobs. The default allowlist is in `src/commands/mod.rs`; it is not an unknown-command fallback. |

Unknown executables and unclaimed Python invocations are errors. External parsers
may retain options they cannot model, but mark uncertain effects rather than guess
option arity or filenames.

### Coan batching

A source stdout job stores input, output, and executable/mode/options separately
from its original argv. Compatible jobs have matching arguments, failure policy,
and relevant stream hints, with independent file paths. A fused stdout batch
copies each input to its output, then invokes coan once with `--replace` and an
stdin filename list. It never opens the original stdout redirect around that
replacement invocation: doing so would truncate a copied output.

Filename-sensitive options, potentially defining source text, unsupported stdin-
list filenames, symlinks, hard-link hazards, conflicts, or incompatible redirects
retain individual execution or produce an explicit safety error. Ignored-failure
stdout jobs stay individual to preserve partially emitted output. Replacement
batches report aggregate coan status; individual command stop boundaries are not
retained inside a fused batch. Coan success accepts its informational/change bits
but rejects error/abort bits, signals, and spawn failures. Non-Unix hosts retain
individual execution; there is no Windows FFI batching subsystem.

## Library surface

The core modules are `shell`, `input`, `registry`, `job`, `graph`, `headers`,
`rewrite`, and `scheduler`. `parse(text, &registry)` returns `Vec<Box<dyn Job>>`;
`run(text, &registry, verbose)` runs the default pipeline. `create_graph`,
`optimize_graph`, and `execute` are also reexported for explicit composition.

`Job: Any + Debug` exposes `data`, `data_mut`, `execute`, and `execute_batch`.
The default batch implementation is exactly:

```rust
items.iter().try_for_each(|item| item.execute())
```

`JobData` contains string `FileAccess` declarations (`Read`, `Write`, `ReadWrite`),
optional `Producer { input, job }` bindings, uncertainty, shell hints, and source
metadata. Implement a concrete `Job`, then register a closure using
`Registry::register(&["executable"], handler)`. The closure receives full argv and
shell hints and returns an optional boxed job. The registry attaches common
metadata and redirect writes. `Registry::empty()` permits a fully explicit set;
`Registry::default()` composes the built-ins.

### Files and ordering

Paths remain original `String`s. Use consistent spelling and forward slashes,
including on Windows. No alias resolution, absolutization, or `..` cleanup occurs.
Component comparisons are for directory hazards only, not rewritten execution
paths. Directory-overlap ordering does not implicitly produce descendant inputs.

Reads select preceding writers unless a generic producer binding supplies a
particular job, including a later job. Read/write, write/write and mutation edges
preserve values; an initial read-modify-write still requires an existing input.
Unknown effects form barriers. Producer bindings cannot silently cross unrelated
barriers: conflicting ordering becomes a cycle before any execution.

`Dag::unproduced` lists initial reads, and `Dag::check_inputs` delegates them to
owning jobs. The default uses `fs::metadata`; ignored-failure jobs defer failures
to execution. `job::PARALLEL_INPUT_CHECKS` is false. Changing that in-code flag
permits Rayon only for existence checks, never for executing commands.

### Optional generated headers

`ENABLE_HEADER_PREPROCESSING` is false. An embedding caller may explicitly use
`headers::discover_headers` and `headers::add_header_dependencies` before creating
the graph. The pass downcasts compiler jobs, inventories existing and declared
header outputs in a trie, and adds ordinary reads/bindings under include dirs.
It skips the compiler's own outputs, chooses the latest preceding or unique
future header producer, and rejects ambiguous future producers transactionally.
It does not parse C includes or walk child symlink directories.

The [Python/header example](examples/python_generated_headers.rs) uses this pass
to hoist a declared generator before a compiler listed earlier in the log, then
runs the built application. Expected output: `generated answer: 42`.

## Windows

```sh
rustup target add x86_64-pc-windows-gnu
cargo build --locked --target x86_64-pc-windows-gnu
wine target/x86_64-pc-windows-gnu/debug/rusty_make.exe --commands commands.txt
```

Cross-building requires a MinGW linker. Native operations and a C-drive command
log have been exercised under Wine 9, including inherited cwd and null streams;
Bash is not required. This is not a claim of full GNU coreutils compatibility or
native-Windows external-tool validation. Wine 9 returned success from directory
symlink creation without materializing a link in the capability probe, so the
Windows directory-symlink removal scenario could not be exercised there.

Real coan tests use `RUSTY_MAKE_COAN=/path/to/coan cargo test --locked`; without an
installed/explicit coan, those reference-tool tests skip. Other installed-tool
checks likewise require their actual executables.
