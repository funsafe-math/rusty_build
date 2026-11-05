# rusty_make

A conservative Rust proof of concept for executing GNU Make command logs with
native filesystem operations, dependency-safe batches, a copy-chain rewrite, and
a persistent content cache. GNU Make expands Makefiles; this tool does not parse
Makefiles or implement a shell. Linux with Bash is the supported execution host.
Windows-looking paths are tested as Bash arguments, **not** as Windows shell syntax.
No Windows performance or compatibility claim is made.

## Build and run

```sh
cargo build
cargo test
cargo run -- --commands commands.txt -j8 --explain
cargo run -- --commands commands.txt --dry-run --dump-plan plan.json --dump-graph graph.dot
cargo run -- --commands commands.txt --compile-commands compile_commands.json
cargo run -- --commands commands.txt --rebuild-all --no-optimize
```

Commands run relative to the **current working directory**, not the log's parent.
Do not replay untrusted logs: external commands have your process permissions.
The whole input is validated and classified before any job executes. A malformed
line, an ambiguous match, or a rejected registered command aborts the entire plan.
Unknown commands print exactly `unknown: <original line>` on stdout and remain
external jobs. Diagnostics, progress and explain output use stderr. Dry-run prints
human-readable waves to stdout; JSON is written only to the requested file.

| Option | Behavior |
| --- | --- |
| `--commands FILE` | Read a UTF-8 command log; otherwise invoke GNU Make |
| `-j N`, `--jobs N` | Maximum worker threads (default 1); must be positive |
| `--rebuild-all` | Ignore incremental hits; independent of optimization |
| `--no-optimize` | Disable graph rewrites, not native execution or batching |
| `--dump-graph FILE` | Graphviz DOT: original identity, type, strategy, dependency reasons |
| `--dump-plan FILE` | JSON: jobs, parsed metadata, edges, waves, versions, rewrite provenance |
| `--compile-commands FILE` | Compilation database using argv arrays, including in dry-run |
| `--python-script ID=PATH` | Repeatable exact script-specific registration |
| `--generated-header-source REGEX` | Repeatable opt-in source predicate for generated-header prerequisites |
| `--explain`, `--verbose` | Classifications, barriers, edges, rewrites, cache reasons, batches, results |
| `--dry-run` | Validate/export/display without executing jobs or writing the cache |
| `--make-arg ARG` | Forward one permitted planner argument (`--make-arg=-f`) |

### Synthetic working build

From the repository root after building:

```sh
cd fixtures/demo
../../target/debug/rusty_make --commands commands.txt -j4 --explain \
  --python-script generate=tools/generate.py \
  --python-script transform=tools/transform.py \
  --compile-commands compile_commands.json
./build/app
# 12
```

Requires `gcc`, `python3`, `sed` and Bash. This exercises all five native utility
families, two Python identities, a text transformation, compilation, linking, an
unknown external command, independent copies and a copy chain. Integration tests
copy this fixture into isolated temporary directories and compare **every regular
file's BLAKE3 hash** against sequential Bash execution and unoptimized execution.

### Python tool with declared generated output

The focused [Python/header example](examples/python_generated_headers.rs) is
runnable from the repository root:

```sh
cargo run --example python_generated_headers
```

It registers a clap-based parser for
[`tools/generate_header.py`](fixtures/python_headers/tools/generate_header.py),
declares the script/template inputs and generated-header output, and runs a real
Python generator plus C compiler in an isolated temporary directory. The fixture
deliberately lists the compiler **before** its generator. The header policy adds
the prerequisite, so Python runs first; the application prints
`generated answer: 42`.

This is an example of a trusted script-specific effect contract, not inference
of arbitrary Python behavior. `--python-script ID=PATH` alone only classifies the
script and remains opaque; precise effects require a registered parser such as
the example's `parse_generator`. Do not register both matchers for the same
command: ambiguity remains fatal.


### GNU Make planning

```sh
rusty_make MY_PARAM=x -j8
rusty_make --make-arg=-f --make-arg=Build.mk all -j8
```

Positional Make arguments and repeated `--make-arg` values retain their original
relative ordering, so later variable assignments take precedence as in GNU Make.

The wrapper runs `make -nB --trace -j1`, streams ordered stdout while retaining it,
and returns a parsed log only after Make exits successfully. Rust's `-j8` does not
reach Make. A conservative option allowlist rejects directory changes, overriding
parallelism, execution modes, arbitrary evaluation and Make control-variable
injection. Make environment controls are cleared; diagnostics use `LC_ALL=C`.
Original target identities from GNU Make traces are attached to jobs. Recognized
Make diagnostics are retained separately, never classified as unknown utilities.
Directory-scoped recursive logs are explicitly rejected rather than run in the
wrong directory. Stderr is inherited; ordering between stdout and stderr is not
claimed.

**Make dry-run is not side-effect-free, even with `--dry-run` on rusty_make.**
`$(file ...)`, `$(shell ...)`, Makefile regeneration, `+` recipes and recursive Make
can execute while generating the plan, including before a failed plan. Generated
files from successful planning are available to jobs. Forced recipes might be
replayed twice; Makefiles requiring that workflow are not safely supported. The
wrapper warns before launch. Use trusted Makefiles, preferably in an isolated
build tree. See [the detailed planning contract](docs/coreutils.md#gnu-make-wrapper-contract).

## Architecture

```mermaid
flowchart LR
    A[Command log or successful GNU Make output] --> B[Input records and trace metadata]
    B --> C[Regex registry and static Bash parsing]
    C --> D[Typed jobs and declared effects]
    D --> E[Hazard DAG and logical versions]
    E --> F[Graph rewrites]
    F --> M[Opt-in generated header prerequisites]
    M --> G[DOT JSON compilation database]
    M --> H[Topological waves]
    H --> I[Content cache decisions]
    I --> J[Typed batches on bounded workers]
    J --> K[Native filesystem or original Bash command]
    K --> L[Successful cache records]
```

* `input.rs`: physical source line and original command spelling; Make target
  context; active Bash backslash-newline continuations. Open quotes, heredocs and
  compound constructs spanning independent physical records are rejected. One-line
  compound commands may remain external. CRLF records work; CRLF continuations do
  not masquerade as Bash continuations.
* `shell.rs`: **tree-sitter-bash** identifies shell structure and validates the
  whole tree; **shell-words** decodes only proven static words. This was chosen
  over a shell interpreter because the PoC needs syntax boundaries and can defer
  expansion to Bash. Tree-sitter is error-recovering, not an authoritative Bash
  validator, so opaque constructs also receive `bash -n` syntax-only validation.
  No shell expansion runs during parsing. Static commands incur no syntax-check
  subprocess. Parser reference: [tree-sitter-bash Rust API](https://docs.rs/tree-sitter-bash/0.25.1/tree_sitter_bash/).
* `registry.rs`: deterministic registered regex order. **Every matching regex
  counts**, including overlaps within one handler; there are no priorities or
  specificity rules. Exactly one match dispatches the typed parser; rejection is
  fatal. Valid complex syntax remains original-text external work with a barrier.
* `commands.rs`, `job.rs`: clap-derived argument structures and an enum of native
  operations. The enum is the simple type-erased, serializable job representation;
  string identities distinguish configured Python scripts without changing the
  scheduler. Identity and execution strategy are separate. `External`, `Internal`
  and `Batched` strategies are represented; this PoC does not fabricate subprocess
  batching for compilers. Original argv/text and parsed flags are preserved.
* `binutils.rs`, `binutils/inspection.rs`, `binutils/mutation.rs`: declarative
  option subsets infer tool inputs/outputs independently of process execution.
  Known effects become ordinary DAG nodes; uncertain modes retain a diagnosed
  passthrough barrier. File redirection alone does not force a barrier.
* `graph.rs`: original-order RAW, WAW and WAR edges; ancestor/descendant path
  conflicts conservatively order directory operations. `mkdir -p` includes
  implicit parent effects, including prerequisites for sibling outputs.
  Unknown-effect jobs normally act as global barriers; explicit generated-header
  prerequisites can authorize a known producer before its selected compiler.
  Unrelated unknown ordering is retained. Independent operations share waves. Existing
  symlinks are resolved for hazard comparison; existing hardlink aliases force
  barriers. POSIX backslashes are never normalized into separators. Logical
  `path@version` reads and writes aid inspection; files are not snapshotted, so
  WAR edges protect readers from later overwrites. Kahn sorting reports cycles.
* `scheduler.rs`: deterministic waves, then compatible identity/strategy groups.
  Each group is one dispatch; members of the same group currently execute
  sequentially. Different groups can run concurrently up to `--jobs`, including
  precise external binary-tool jobs. Native groups avoid subprocesses. Compiler
  and opaque jobs retain conservative ordering except for explicit selected
  generated-header prerequisites. First failure cancels not-yet-started work;
  already-running independent
  work finishes, no dependent wave starts, and the process exits nonzero.
* `rewrite.rs`: extensible `GraphRewrite` trait plus a real adjacent copy-chain
  rewrite. `cp A B; cp B C` becomes one two-output operation. **B is preserved**,
  even if it is a final artifact or has other consumers. For ordinary distinct
  new output files it reads A once and writes both outputs with a bounded 64 KiB
  buffer. Existing/aliased/unsupported paths fall back to the original two-copy
  sequence. Source overwrite patterns are rejected; all original jobs remain in
  `provenance`. Future fusion/compile batching/jumbo passes can implement the same
  trait; none are advertised as implemented.

The graph uses pairwise hazard comparisons (quadratic in command count), not a
fully sequential chain. This favors a small, auditable implementation over a
premature indexed dependency engine. No throughput/speedup benchmark is claimed.

### Why job behavior is not a trait yet

The PoC stores serializable `Job`/`JobSpec` records and uses an `Operation` enum
for native execution. That keeps fingerprints, JSON, provenance and rewrite
matching simple, but adding native behavior still requires editing the enum and
executor dispatch. Parser callbacks already allow additional external tools
without scheduler changes. File-per-command modules are possible with either
representation; they do not require a trait.

For further native extensibility, the preferred direction is serializable records
plus trait-based command implementations and typed registration with an erased
batch dispatcher. A static `execute_batch(&[Self]) where Self: Sized` cannot be
called through `dyn Job`, so a heterogeneous trait-object vector alone does not
solve batching. That behavior-layer refactor is **not implemented** in this PoC.


## Supported command subsets

| Type | Implemented behavior | Conservative boundary |
| --- | --- | --- |
| `cp SRC DST` | In-process regular-file copy; existing destination permissions retained; new mode respects umask | Exactly two operands; directories, symlink operands, special bits and multiply-linked destinations rejected; no flags |
| `mkdir [-p\|--parents] PATH...` | Native creation; intermediate owner write/search for `-p` | Mode options rejected; overlapping directory effects ordered |
| `touch PATH...` | Native creation or atime/mtime update | Ordinary files only; no options; never skipped |
| `rm [-f\|--force] PATH...` | Native unlink; force ignores missing paths | No recursion/options beyond force; global barrier; never skipped |
| `mv SRC DST` | Native regular-file rename | Same filesystem; no options, directory/symlink/multiply-linked destination behavior; barrier; never skipped |
| `sed` | Typed `-e`, `-f`, `-n`, `-i[SUFFIX]`, operands; original command executed | No native sed/semantic inference; arbitrary scripts are global barriers and never cached |
| GCC/Clang/`cc`/C++ drivers | Typed include/library paths and common flags; partial source/output metadata | Unknown flags/response files preserved; conservative effects and no cache hits; explicit generated-header policy can order known prerequisites |
| Binary inspection and manipulation tools | Separate identities, original argv/redirections, typed file-effect subsets; GNU/LLVM/cross-prefixed names recognized | Known forms use ordinary file hazards, not global barriers; uncertain forms remain passthrough barriers; no native implementation or cache hits |
| Configured Python scripts | Independent identities across `python`, `python3[.N]`, `py`, absolute POSIX interpreter paths | No guessed script dependencies or interpreter options; original-text execution/barrier |
| Other programs | Visible `unknown:` external jobs | Unknown filesystem effects ordered against all other jobs |

Compiler include and library search paths support both separated and joined
forms: `-I include`, `-Iinclude`, `-L lib`, `-Llib`, including quoted paths with
spaces. `-isystem`, `-iquote` and `-idirafter` paths are also extracted (separate or
joined) into include-directory membership metadata. Search order remains in
original argv. Unknown options such as `-Wall`, `-std=c11`, `-lfoo` and extended
`-g`/`-O` forms do **not** abort planning. The original command executes unchanged,
and compilation database entries preserve every argument. Only the compiler
decides whether forwarded flags are valid; a compiler failure stops dependent work.

JSON metadata exposes `includes`, `library_paths`, `unparsed_args` and
`metadata_complete`. Unknown option arity is not guessed: following bare operands
stay opaque until another option boundary, so option values like
`-include generated.c` are not mistaken for compilation sources. Response-file
contents are likewise opaque. Sources hidden in those opaque operands are omitted
from compilation database entries rather than invented. Default output filenames
are not inferred when unknown arguments could change the compiler's mode;
explicit `-o` metadata is retained. Missing values for recognized flags remain
parsing errors. `--explain` reports partial metadata and preserved arguments.

Binary tools are registered for `nm`, `objdump`, `readelf`, `size`, `strings`,
`addr2line`, `c++filt`, `objcopy`, `strip`, `ar`, `ranlib`, `as`, `ld`, `elfedit`
and `dwp`. LLVM equivalents and `llvm-readobj`, `llvm-dwarfdump` and
`llvm-symbolizer` are recognized, including `llvm-cxxfilt`. Matching also covers
target prefixes (`arm-none-eabi-nm`), absolute/quoted POSIX executable paths,
numeric version suffixes (`llvm-objdump-18`), lowercase `.exe` spellings, and
`ld.bfd`, `ld.gold`, `ld.lld`, `gold` and `lld`. Recognition of `.exe` names is not
Windows shell support. LLVM names are classification-tested; runtime verification
uses the installed GNU tools.

All options are passed through unchanged, including options unknown to rusty_make.
The executable must be installed; its exit status determines success. External
execution is **not** an ordering barrier by itself. Understood invocations retain
their explicit inputs/outputs and may share waves with unrelated work.

Common precise forms include:

* `nm` file operands (default `a.out`), `objdump` header/section/symbol modes,
  `readelf` inspection, `size`, `strings` files or redirected stdin, `c++filt`
  positional symbols or redirected stdin, and supported `llvm-readobj` switches.
* `objcopy` explicit/in-place outputs, including add/update-section file inputs
  and multiple dump-section outputs; `strip` explicit/in-place output; normal
  `ar r[c/s/D]` archive updates; in-place `ranlib`/`elfedit`; explicit `dwp` inputs
  and `-o` output.

Only real RAW/WAR/WAW/directory hazards order these jobs. Two inspectors may read
the same object concurrently; an in-place mutation waits for earlier readers.
Appending to a shared report remains a read/write dependency. `--explain` and
JSON `effect_reason` distinguish precise effects from fallback reasons.

Unknown flags and response files still execute unchanged, but their effect set
is not guessed. Plugins, source/line modes, inherited stdin, assembly includes,
linker scripts/libraries and debug discovery remain barriers. In particular,
ordinary `objdump -d`/`-D` may follow separate debug files; an explicit
`--dwarf=no-follow-links` permits precise disassembly. `addr2line`, symbolizer and
dwarfdump modes with hidden debug inputs remain conservative. No flag is inserted
into the original command to change its semantics.

The precise subset assumes ordinary non-aliased regular files and stable tool
configuration. Existing symlink/hardlink operands and thin archives fall back to
barriers. If an earlier job creates one of these unsupported states after
planning, a runtime guard refuses the precise invocation before opening its
outputs and requests replanning; a new plan can then use a barrier. This avoids
silently executing against stale path or archive-member dependencies.

```sh
nm -n build/app > build/app.nm
objdump -d --dwarf=no-follow-links build/app > "build/app disassembly.txt"
readelf -a build/app >> build/inspection.txt
objdump -d build/app > build/app.dump 2> build/objdump.err
c++filt < mangled.txt > demangled.txt
```

Every tool supports applicable Bash file redirections, including stdout/stderr
overwrite and append, quoted filenames, stdin, and descriptor combinations such
as `> report.txt 2>&1`. Parent directories must already exist. Simple redirection
paths appear as known inputs/outputs in the JSON graph; append outputs are also
read/write artifacts. Multiple or ordering-sensitive redirects remain opaque but
are honored by executing the exact original command, never by rebuilding argv.

Simple stdin/stdout/stderr redirects and stdout append are recognized separately
from argv. Multiple/descriptor redirects and dynamic words, globbing, assignments,
pipelines, substitutions, loops and shell lists stay opaque. A simple redirection
adds file effects to a precise external job without discarding its tool inputs.
Native redirection-bearing utilities still use a conservative shell fallback;
no native handler drops shell file-opening effects. External execution uses
`bash --noprofile --norc -c` with `BASH_ENV` and `ENV` removed. Opaque programs
retain their original text and a barrier rather than misleading partial argv.

Native support is deliberately not a replacement for GNU's complete semantics:
ACLs, extended attributes, ownership preservation, sparse-file heuristics, unusual
filesystem races and timestamp-sensitive workflows are outside the supported
native subset. Native copy/mkdir/touch also reject symlink path components at
execution time: an earlier opaque job must not introduce aliases that invalidate
the planned hazards. Do not run another filesystem-mutating build concurrently
in the same root. A `.rusty_make/build.lock` excludes simultaneous rusty_make
executors; after a crash, verify no build is running before deleting a stale lock.
No transactional rollback is promised for a failed batch.

## Conditional generated-header dependencies

```sh
rusty_make --commands commands.txt \
  --generated-header-source '^src/special\.c$' --explain
```

The option is repeatable (patterns are ORed), disabled by default, and independent
of `--no-optimize`/`--rebuild-all`. It matches original compiler source-path
spellings, including source-bearing compile-and-link jobs.

The library accepts an arbitrary predicate with access to the job:

```rust
use std::path::Path;
rusty_make::headers::add_generated_header_dependencies(
    &mut graph,
    |_, source| source == Path::new("src/special.c"),
)?;
```

For every selected job, the policy scans **all declared plan outputs** for headers
beneath any of its `-I`, `-isystem`, `-iquote` or `-idirafter` directories. Paths are
normalized against the graph cwd and compared by components, so `include-extra`
does not match `include`. It adds each matching header as an input and **every
writer** as a producer prerequisite. Nonmatching compiler jobs are unchanged.

Header recognition uses case-insensitive suffixes `h`, `hh`, `hpp`, `hxx`, `h++`,
`inc`, `inl`, `ipp`, `tpp`, `tcc`, `cuh`. Self-output paths are excluded. This is
deliberately an overapproximation of generated headers, not preprocessing or
complete compiler dependency discovery. Implicit environment/sysroot searches,
opaque shell expansions and headers hidden in unknown option operands are not
resolved. Compiler jobs remain non-cacheable.

There is no filesystem crawl or guessed Python output. A generated file must
appear in `JobSpec.outputs`; the Python example shows that contract. Files created
internally during Make planning already exist before execution but have no replay
producer unless represented in the plan.

Requirements bind final generated-file versions and use original source-line
identities, retained through copy-chain fusion. A later **known-effect** producer
can move before the selected compiler. Real conflicting data hazards, unknown
future producers and unrelated unknown barriers are not silently overridden:
incompatible requirements report a cycle before any job runs. Policy failure
rolls back the graph. `--explain`, DOT and JSON show the new prerequisites;
JSON retains them in `required_dependencies`.

## Incremental builds

`.rusty_make/cache.json` stores schema/version, stable source position/target,
command/parsed-operation fingerprints, cwd/environment fingerprints, and explicit
input/output BLAKE3 hashes plus permission bits. Files are rehashed every time:
there is no size/mtime shortcut. Directories use an existence/mode marker, not a
recursive hash (creating another child does not invalidate `mkdir -p`). Symlinks,
special files and hardlink aliases do not produce safe cache hits.

Only precisely modeled native copies/copy chains and mkdir jobs are cacheable.
Touch is observable timestamp work and is always executed. Compiler headers,
Python scripts, sed programs and unknown commands may hide arbitrary dependencies:
these jobs and jobs with unknown-effect ancestors are not skipped. GCC `-MMD -MF`
depfiles were considered but are **not implemented**; it would be unsafe to cache
compiler jobs based only on command-line sources.
Known-effect external tools also remain non-cacheable until executable/tool
configuration fingerprints are implemented; that restriction does not serialize
them. When such a tool reruns and emits identical bytes, a cacheable downstream
native copy can still skip.

Cache evaluation happens in dependency order against current file contents. If an
upstream job runs because its arguments changed but writes identical bytes/modes,
a precisely modeled downstream job can still hit its cache. Missing/tampered
outputs and modified inputs force rebuilds. `--rebuild-all` bypasses every hit.
Records are invalidated and saved before launch, then recorded only for successful
completed jobs. Failed/incomplete jobs cannot retain an old success entry. Cache
writes use a synchronized temporary file and rename; interrupted writes may leave
`.rusty_make/cache.json.tmp`, which is explicitly reported instead of overwritten.
The cache is trusted build state, not an adversarial integrity boundary.

## Graph-aware compilation database transforms

The CLI emits standard `compile_commands.json` entries with `directory`, `file`,
`arguments` and optional `output`. Original argv is used rather than a quoted
shell string. One entry is emitted per recognized source; pure linking is omitted.
Compile-and-link invocations with sources are included. Opaque compiler shell
commands are not flattened into misleading entries.

The library exposes the holding object and the graph **before saving**:

```rust,no_run
use rusty_make::compdb::CompilationDatabase;
# fn export(graph: &rusty_make::graph::BuildGraph) -> anyhow::Result<()> {
let mut database = CompilationDatabase::from_graph(graph)?;
database.save_with_transform(graph, std::path::Path::new("compile_commands.json"),
    |graph, database| {
        // Inspect graph.jobs, graph.edges, versions and original provenance.
        for entry in &mut database.commands {
            if graph.jobs.iter().any(|job| job.source.target.as_deref() == Some("generated")) {
                entry.arguments.push("-DGENERATED_BUILD".into());
            }
            entry.directory = "/relocated/build".into();
        }
        Ok(())
    })?;
# Ok(()) }
```

Transforms may filter/add entries or modify fields. A transform error aborts before
touching the destination. The hook is a Rust library API, not evaluation of
arbitrary code from CLI configuration.

## Extending handlers

1. Define clap-derived arguments and a `fn(&ShellCommand) -> anyhow::Result<JobSpec>`.
2. Populate typed parsed metadata, exact file effects and cacheability. Default to
   `unknown_effects: true`, `cacheable: false` if there is any uncertainty.
3. Register one or more anchored regexes with `Registry::register`. Any overlap
   remains fatal; parsers do not resolve ambiguity. `Registry::add_python` is the
   supplied exact-script registration mechanism.
4. Reuse `Operation::External` for metadata-attached fallback behavior, or add a
   native enum variant and its execution branch. The scheduler needs no changes.
5. Test classification, unsupported flags, semantics, hazards and real execution.

For another opaque binary tool, use
`Registry::register_external("tool-name", &[r"^tool-name(?:\s|$)"])`. This preserves
all arguments and shell semantics without writing a flag parser; unknown effects
still force a barrier. Its patterns obey the same fatal ambiguity rules.


[Coreutils investigation](docs/coreutils.md) records reusable uutils crates, their
in-process APIs, process-global concerns, Windows status, GNU compatibility, MIT
license, dependencies and batching practicality. Individual lower-level APIs can
be reused, but CLI entry points are not assumed reentrant. The PoC chose narrow
standard-library operations rather than inherit CLI/global-state machinery.

## Validation and next improvements

Tests cover registry ambiguity (including within one handler and Python overlaps),
quoting/operators/paths, Make traces and continuations, RAW/WAR/WAW/directory
hazards, versions, cycles, barriers, typed batches, failure cancellation, copy-chain
differential execution, cache invalidation/content propagation, compilation
database transforms and real Make planning. Optional compiler/Make integration
tests skip with a message if their external tool is absent. No original Windows
project is required. Run `cargo build` and `cargo test` on the Linux host.

Verified on Linux with Rust 1.94.0, GNU Make 4.3 and GCC 13.3:

* `cargo build`: successful.
* `cargo test`: **114 passed**, no failures, including real compiler, Python, sed,
  Make and differential fixture execution.
* Standalone CLI fixture: 15 commands became 14 nodes with one copy-chain rewrite;
  generated application printed `12`. DOT, plan JSON and a two-entry compilation
  database were written and inspected.
* A standalone two-copy log executed once as a fused node, then executed **zero**
  jobs on the identical second run. `--rebuild-all --no-optimize` executed both
  original jobs. The real Make CLI smoke preserved expansion-generated files and
  copied the planned output correctly.
* Compiler passthrough CLI smoke: joined/separated `-I` and `-L`, paths with
  spaces, and unknown-to-Rust flags (`-Wall`, `-Wextra`, `-std=c11`, `-lvalue`)
  compiled and linked against a generated static library; the application printed
  `12`. Exported plan metadata and original compilation argv were inspected.
* Binary-tool CLI smoke: 17 commands compiled, inspected, archived, copied,
  stripped, assembled and partially linked an object. Redirected reports included
  appended `nm` symbols and stdin-driven `c++filt` output (`foo()`); `as`/`ld`
  outputs retained the expected exported symbol. GNU inspection output was also
  compared byte-for-byte against direct tool execution in the regression suite.
* Precise-effect CLI smoke: 11 commands completed in four waves, with six jobs
  sharing one ready wave. Known `nm`/`objdump`/`readelf`/`objcopy`/archive effects
  removed blanket serialization while strip/archive consumers kept their file
  dependencies. Tests also cover multiple outputs, WAR hazards, append ordering,
  unchanged-output propagation, and thin-archive fallback/replanning.
* `cargo run --example python_generated_headers`: generated a header with Python,
  hoisted its known generator ahead of the compiler, and ran the application,
  which printed `generated answer: 42`.
* Generated-header CLI smoke: a source regex selected one compiler using
  `-isystem generated`; a later copy ran first, the application printed `42`,
  and an unselected compiler did not gain the header input. This also exercised
  `--no-optimize --rebuild-all`, explain output and all plan/database exports.

Commands and diagnostics both retain their original input line numbers in JSON;
rewrite provenance preserves removed command records. Synthetic planner warnings
have no input line number.


Recommended next work, in order:

1. GCC/Clang depfiles and an audited header/tool/environment dependency model.
2. Proven script-specific Python effects and native implementations for hot scripts.
3. An indexed hazard builder and event-driven ready queue; parallel typed batch shards.
4. An explicit shell/backend and path policy for Windows, verified on Windows CI.
5. Pinned, individually audited uutils lower-level APIs where broader GNU semantics
   justify their footprint; benchmark real workloads before new fusion passes.
6. Recursive Make working-directory/protocol support and planning sandboxing.
