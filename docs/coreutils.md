# Native filesystem handlers

The clean core implements a narrow, explicit native subset using `std::fs`.
It does not link uutils, call a shell, or silently fall back to external coreutils
when a native invocation is unsupported. Clap rejects unsupported options before
any command in the log executes.

| Command | Accepted subset | Operation and limits |
| --- | --- | --- |
| `cp SRC DST` | Exactly two regular-file operands, no utility flags | Copy bytes into the destination. Preserve existing destination permissions/hard links; new Unix files use source permissions filtered by umask. No directory copy, symlink traversal, special files, or same-file copy. |
| `mkdir [-p/--parents] DIR...` | One or more directories | `-p` calls `std::fs::create_dir_all`; otherwise `create_dir`. No manual prefix-by-prefix execution, modes, or verbose flags. Parent prefixes in metadata describe ordering effects only. |
| `touch FILE...` | One or more regular files, no utility flags | Create absent files; set access/modified times on existing files without truncation using `FileTimes`. No timestamp/reference/no-create options or symlink traversal. |
| `rm [-f/--force] [-r/-R/--recursive] PATH...` | Files, symlinks, and explicit recursive trees | Remove links without traversing their targets. `-f` permits absent operands and an empty operand list, not invalid components or other errors. Refuse root, final `.`/`..`, and symlink ancestors. |
| `mv SRC DST` | Exactly two regular-file operands, no utility flags | Same-filesystem `rename`. Reject same-file aliases and directory operands; cross-device moves fail instead of approximating copy-and-delete. |

Use `--` to terminate native options when an operand begins with `-`. Operands and
file-effect declarations retain their original strings. The implementation does
not canonicalize paths, change cwd, convert aliases into alternate execution
paths, or reproduce every GNU semantic. Consistent forward-slash spelling is the
command-log contract, including on Windows.

## Streams and failures

Native jobs open/truncate redirects in their original source order **before**
performing the filesystem operation. A redirect-open failure prevents the
operation; already opened earlier redirects retain their effects. Native commands
produce no stdout. Operation diagnostics go to configured stderr, or process
stderr, with failures returned to the caller. `/dev/null` maps to a native null
stream; Windows also accepts `NUL` and `NUL:`.

A recognized final `|| true`, `|| :`, or `|| cd .` masks runtime failures, including
redirect failures. It does not bypass parsing or allow unsupported native flags.
Without that policy, the scheduler stops on the first failed batch.

## Metadata and preflight

Jobs declare `FileAccess` paths for generic DAG ordering. Copy reads its source
and writes its destination. Move consumes/mutates its source and writes its
destination. Touch can create a file without an initial content read. Removal is
a mutation, not a required content read, so `rm -f missing` remains executable.
Directory ancestor/descendant hazards add ordering edges, but an ordinary directory
write is not an implicit producer of every descendant file.

Before executing any job, the scheduler checks unproduced input reads. A
read-modify-write is not its own producer. Ignored-failure jobs defer input
failures to execution so their explicit failure policy remains useful. No artifact
snapshots, persistent state, cache, or filesystem watcher is involved.

Symlink-component rejection and opened-handle identity checks protect the native
subset from known alias hazards. The portable `same-file` crate supplies file
identity; there is no handwritten Windows FFI. These checks are not a sandbox or
protection against arbitrary concurrent filesystem mutation.

## Verification and platform limits

The native suites cover bytes, timestamps, recursive trees, alias rejection,
missing operands, invalid parent components, redirect timing, failure policy,
and dependency ordering. Integration tests launch the actual CLI with an
inherited working directory. Linux smoke execution additionally exercised actual
C compilation, sed, gawk, and coan.

Windows GNU-target tests and actual C-drive native execution were exercised under
Wine 9. Windows can report a child of a regular file as `NotFound`; removal checks
parent components so `-f` cannot accidentally mask that error. The directory-
symlink removal test remains in the suite for a capable Windows runtime. Wine 9's
symlink creation probe returned success without creating a link, so that scenario
was excluded from the supported Wine run, not treated as a verified removal.

External-tool jobs remain external processes with original argv and inherited
cwd. They require host-compatible tool installations. Non-Unix coan execution
stays individual rather than introducing platform-specific batching machinery.
