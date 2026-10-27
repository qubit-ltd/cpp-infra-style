# cpp-infra-style

[简体中文](README.zh-CN.md)

`cpp-infra-style` checks every configured C/C++ source file before a build. It runs
clang-format using one explicit organization policy and checks SPDX headers,
final newlines, and trailing whitespace. New, untracked files participate.
It does not require CMake or a compilation database.

Requires Rust 1.94 for building and clang-format 18 for checks. The shipped policy
uses LLVM formatting, two-space indentation, a 100-column limit, LF line endings,
an `Apache-2.0` SPDX identifier, and a `Copyright (c)` comment in the first eight
lines. Project and policy paths are resolved inside the canonical project root.

```sh
cargo build --locked
cpp-infra-style --project /path/to/project plan
cpp-infra-style --project /path/to/project check
cpp-infra-style --project /path/to/project fix --dry-run
cpp-infra-style --project /path/to/project fix
```

The project must have `.infra/project.toml`:

```toml
schema = 1
kind = "library"
source_roots = ["src", "include"]
test_roots = ["tests"]
generated_roots = []
third_party_roots = []
```

Declare only existing directories, with no overlapping ownership. Configure
`.infra/style/config.toml`:

```toml
schema = 1
extensions = ["h", "hpp", "hxx", "c", "cc", "cpp", "cxx", "ipp", "tcc"]
include = ["src/**", "include/**", "tests/**"]
exclude = []
rules = ["format", "spdx-header", "final-newline", "trailing-whitespace"]
exceptions = ".infra/style/exceptions.toml"
```

The tools bundle installs `conf/policy/rules.toml` and `clang-format.yaml` under
`.infra/style/policy/`. Its `project_files` manifest also declares `conf/config.toml`
as the initial `.infra/style/config.toml`: `init` creates it only when absent,
and normal updates preserve project edits. The shipped include pattern `**`
intersects the actual project ownership roots, without assuming directory names.
Project configuration cannot remove organization-required
rules. Nested `.clang-format` files do not change this tool's behavior. Scope is
the intersection of project source/test roots and `include`, minus `exclude`.
Generated and third-party ownership roots are excluded. `.git`, `.infra`, `build`,
and `target` are excluded only at the project root; nested source directories
with those names remain in scope. In-project symlinks are not followed; escaping links fail. Git ignore
rules are not consulted. An empty selection or non-UTF8 source fails.
On POSIX, literal backslashes in project input names are rejected rather than
being converted into directory separators or aliases of exception paths.
Active project, style, policy, formatter, and exception configuration must be
regular files covered by the shared fingerprints. Their lexical and canonical
locations cannot be under root `.git`, `build`, `target`, `.infra/.tmp`,
`.infra/transactions`, or `.infra/.transactions`. File and parent-directory
symlinks in active configuration paths are rejected because the shared no-follow
fingerprints do not track link identity. This restriction does not apply to
runtime contexts, snapshots, or generated reports. Regular custom exception files
under tracked locations, including nested `src/build`, remain supported.
The loader retains each consumed configuration file's identity and byte digest.
Checks before and after formatting, before replacement, and during the final
check reject runtime policy changes, including external custom exception files.
`fix` never absorbs those changes into its refreshed source identity or reloads
new exceptions to certify an old context. A selected file that is also active
configuration is not rewritten by `fix`.

An optional exact exception file looks like:

```toml
schema = 1
[[exceptions]]
rule = "spdx-header"
path = "src/compat.cpp"
reason = "Original copyright and licensing must remain intact."
```

Exceptions require a known, enabled, exemptible rule, an existing literal path,
and a nonempty reason. `format` cannot be exempted. Unknown fields, wildcard
exceptions, duplicate entries, and stale exceptions matching no diagnostics fail.
Reports retain suppressed diagnostics and match counts.

`check` never writes source. `fix` only applies clang-format, preserves permissions,
and compares original bytes before each atomic file replacement. An intervening
edit aborts without overwriting that edit. Multiple files are not a single
transaction: reports list completed and unfinished files. After replacement, a
full check runs again; remaining mechanical violations still fail. Dry-run prints
a unified patch and does not edit source. Formatter failure cannot be suppressed.

Every invocation writes a shared protocol-1 report, by default at
`build/infra-runs/<run-id>/style-report.json`. `--report <path.json>` changes the
report destination. Command logs and patch artifacts remain under `report_root`
with SHA-256 digests.
Each invocation owns an exclusively created `style/<unique-call-id>/` directory
under that shared root. Logs and patches never replace artifacts from another
task or retry. Unsafe output ancestors are rejected before creating files;
when no safe report destination is available, the error is written to stderr.
`--run-context <json>` validates canonical project identity, the current runtime
bundle identity, source digest and configuration digest. A stale context fails.
Optional `source_excluded_paths` metadata contains sorted, deduplicated exact
project-relative JSON report files. A project-local `--report` outside root
`build`/`target` is added after validating it is a non-input report location.
Source, test, generated, third-party, infrastructure, CMake, Conan, and README
inputs cannot be excluded. Existing files must be public cpp-infra reports;
unknown business JSON is preserved. The same list is used for initial and final
source identity; configuration identity never uses it. Only root
`.infra/.tmp`, `.infra/transactions`, and `.infra/.transactions` are ignored as
temporary transaction directories in configuration identity.
The runtime bundle may follow cpp-infra main without matching the project's
persistent tools lock; project infrastructure changes only on explicit update.
When context includes `runtime_snapshot` and `runtime_snapshot_digest`, the tool
verifies snapshot SHA-256, bundle identity, its own version/revision/protocol, and
both the cached and currently running executable digests. A copied executable in
`CPP_INFRA_BIN_DIR` may have a different path when its bytes match the snapshot.
`--offline` never triggers network access. `--timeout-seconds` bounds each
formatter process (default 30). Ctrl-C stops the formatter process group on Unix
and uses `taskkill /PID <pid> /T /F` on Windows to terminate its process tree,
then writes a cancelled report. `CPP_INFRA_CLANG_FORMAT` selects a formatter
executable; otherwise it is found as `clang-format` on PATH. Version checks still
apply. `--version --json` reports `revision: null` for unpublished source builds.

Exit codes: 0 passed, 1 style gate failed, 2 configuration/protocol/version error,
3 filesystem/process failure, 130 cancelled. `plan` prints JSON without launching
external tools and records mode `plan`; it does not certify a style check.

```sh
cargo test --locked
cargo +nightly fmt --all -- --check --config-path .infra/style/rustfmt.toml
cargo clippy --all-targets -- -D warnings
```

Tests use isolated projects, real clang-format 18 for normal workflows, and
controlled subprocess fixtures for crash, timeout, and concurrent-edit behavior.
No downstream repositories are tested.
