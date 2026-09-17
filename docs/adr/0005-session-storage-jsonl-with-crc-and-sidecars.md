# Session storage: pi-style JSONL + CRC + sidecar blobs

**Context**: the user's rule was to deviate from pi's JSONL only if a candidate is more efficient across the ticket's dimensions (write/read/branch/compaction cost, size, crash safety, readability). Research #3 measured a ~1 GB corpus of real pi sessions: the encoding gap is 4–14% (shrinking to ~7% on payload-dominated sessions); the size headroom is in the *payload* (base64 images were 40.4% of the largest session), not the encoding. Follow-up measurement 2026-09-16: zstd -3 compresses a 48.9 MB session to 14.2 MB (3.4×) — so a smaller format exists, but compressing the log itself kills O(1) append and human readability, the two properties the ticket weights.

**Decision**: keep the pi-style JSONL family — one file per session; header line; append-only entries `{type, id, parentId, timestamp, …}`; a branch is an appended child line (never a rewrite); compaction/OM records are appended entries referencing the replaced span. Plus three hardenings:

1. **Per-line CRC** field (measured +0.2% of file size) — detects silent mid-line corruption that parse-or-skip cannot.
2. **Out-of-band sidecar blobs** for payloads above a threshold: raw bytes (removing base64's 33% expansion), zstd-compressed, referenced by id from the entry — the log stays readable, the worst-case single write shrinks, and tail reads stay cheap.
3. **Manual archives** (explicit user action — no auto-archive, no auto-compression, no cleanup schedule): a session file is zstd-compressed into `archive/` and removed from `sessions/`; un-archiving decompresses it back (one-way per action, off the live read/write path) — captures the 3.4× reclamation where it actually matters (old sessions), on demand.
4. **Placement** (user-confirmed 2026-09-16): workspace-scoped session data lives in the workspace's project directory — `{project root}/.tau/sessions/` (one file per session, sidecar blobs in `blobs/`, manual archives in `archive/`) — alongside the per-workspace task store at `{project root}/.tau/tasks/`; sessions with no project live in `~/.config/tau/sessions/`. Placement is a core-side rule: on a remote backend the files live on the host where tau-core runs (no local paths as identity, #13).

**Consequences**: live session files stay `grep`/`jq`/`vim`/`diff`-able — a first-class debugging artifact for a dev tool; total on-disk footprint ends up comparable to a fully compressed log, with the live log readable.

**Considered** (all rejected by the rule): MessagePack/CBOR (−4…14%, unreadable, no structural gain); SQLite WAL (fsync per commit, opaque files, VACUUM-only reclamation, no concurrency gain for per-session files); segmented LevelDB-style log (rotation machinery is overkill at ≤85 MB scale — the CRC idea is borrowed instead); whole-file or segmented zstd log (3.4× smaller, but non-appendable or tool-only inspection).

**Revisit triggers**: sessions routinely >~100 MB, multiple processes writing one session file, or structured queries across many sessions → SQLite (WAL) or a segmented log become worth revisiting.
