# DELETE-ME: Stage-2 handoff — finish the Glove80 lighting observability + attestation work

Written 2026-08-06 by the coordinating Fable session on jay-lenovo, for a
fresh Fable session on ryzen-shine. This branch and this directory are
throwaway: the work lands elsewhere (see "Where things land"), and this
branch must be deleted afterwards. Everything you need that is not in a
repository proper is in this directory.

## Mission in one paragraph

Ivan's Glove80 runs a downstream RMK stack. The right (peripheral) half's
battery-gauge lighting used to wedge permanently on/off; that bug is
DIAGNOSED AND FIXED (four liveness defects in the split-lighting
replication, fixed board-side and flashed 2026-08-06 — see
`wedge-diagnosis.md` here, and glove80-rmk master commits `41168e2..df9126f`
plus `07edc50`). Remaining work, in order: (1) land the host-visible
observability protocol this very branch carries, through the fork-fold
assembly workflow; (2) implement the board/CLI side in glove80-rmk;
(3) implement digest attestation per the committed design doc; (4) flash
and run a fully host-driven verification matrix. A longer-term upstreaming
plan for a generic sync primitive exists but is NOT this handoff's job.

## The three-repo layout

1. **glove80-config** — Ivan's outer config repo,
   `github.com/colonelpanic8/glove80-config` (private; on jay-lenovo it is
   `~/Projects/glove80-config`). Holds the runtime keymap/lighting config
   (`config/glove80.toml`), compiled defaults (`config/firmware.toml`), the
   flash tooling (`bin/glove80-safe-flash`, `bin/glove80-control`), and the
   `dependencies/glove80-rmk` submodule pin. Read its `AGENTS.md`. Outer
   master is pushed through the repin of glove80-rmk `8861bd0`.
2. **glove80-rmk** — the firmware repo,
   `github.com/colonelpanic8/glove80-rmk`, master at `8861bd0`. Board crates
   (`crates/glove80-rmk`, `crates/go60-rmk` — go60 `#[path]`-includes the
   glove80 lighting modules, so keep it compiling: `just go60-dist` with
   `GO60_ALLOW_DIRTY=1`), the CLI (`crates/glove80-control`), and two nested
   submodules: `dependencies/rmk` (pinned to the generated `assembled`
   branch of `colonelpanic8/rmk`) and `dependencies/rmk-assembly` (the
   fork-fold manifest). Read its `AGENTS.md` — with one correction: the
   lighting topic is NOT `fold/lighting-rynk` (deleted); it is the
   manifest's `pr = 1031` entry, branch `glove80-rmk/lighting-v2` on
   `colonelpanic8/rmk`, upstream PR https://github.com/HaoboGu/rmk/pull/1031.
3. **rmk** (this repo) — `colonelpanic8/rmk`, fork of HaoboGu/rmk. THIS
   BRANCH (`DELETE-ME/stage2-fable-handoff`) sits on top of
   `glove80-rmk/lighting-v2` (tip `dfdafb17`) plus three real commits of
   observability work (see below) plus this handoff commit. The `assembled`
   branch is fork-fold OUTPUT — never commit to it, never base on it.

## What this branch carries (the "data exchange" work)

Three commits on top of lighting-v2, written by a prior implementation agent
and verified by the coordinator:

- `0ce05f06` feat(lighting): read back the frame the output presented
- `ace9f676` feat(rynk): add lighting frame and replica-status readback
  (new Rynk commands `GetLightingFrame`, `GetLightingReplicaStatus` + payloads)
- `f3de72e3` feat(rynk): serve the frame and replica-status endpoints
  (host-service wiring, board-facing ports, loopback integration tests)

Verified so far: `rmk-types` native suite 95/95 (wire snapshots regenerate
cleanly — the protocol reference test passes), `rynk_lighting` integration
5/5 including the new observability loopback test. NOT yet run: the
`.github/ci/test.sh` cross-feature nextest matrix, clippy, no_std target
checks. That is your first task.

Read the three diffs before anything else — the board-facing port types
they export (remote-frame port, replication-status port, and the
controller's builder hooks) are the API glove80-rmk must implement in
phase B. The exact names live in `rmk/src/host/rynk/lighting.rs`.

## Context documents in this directory

- `wedge-diagnosis.md` — the full file:line diagnosis of the replication
  wedge (W1–W5), ApplyReplica error enumeration, and the proof that
  `ChargeState::Charging` was unreachable. Explains WHY every already-landed
  fix is shaped the way it is. The fixes themselves are on glove80-rmk
  master; do not re-implement them.
- `split-app-transport-report.md` — verified semantics of the split-app
  channel: 26-byte payload ceiling, queue depths (TX 112 / RX 8 / periph-TX
  2), drop points, `Watch` link-edge collapse, BLE loss vectors. Your
  phase-B transport design must respect all of it.
- The committed (non-throwaway) design doc for attestation and the eventual
  generic primitive is `docs/replicated-half-state-sync.md` **in
  glove80-rmk master** — the digest rules there are binding for phase C:
  hash canonical wire encodings via the same walk that sends; scene/overlay
  domains are maps (order-independent fold allowed); the conditional table
  is a SEQUENCE (later-wins order is semantics — hash in order); central
  digests the right-half projection only; peripheral recomputes from
  applied state, never incrementally; fast-moving context is excluded from
  digests and covered by revision/seq freshness.

## Current hardware/firmware state (on jay-lenovo)

The physical keyboard is attached to jay-lenovo, NOT ryzen-shine. Both
halves run glove80-rmk `df9126ff` (all four wedge fixes + the VBUS→charging
proxy), runtime config applied clean. If you cannot reach the hardware,
do phases A–C and stop before D with everything build-verified; the
coordinator on jay-lenovo (or Ivan) runs phase D with
`bin/glove80-safe-flash` (crash-loop-safe; `--peripheral` for the right
half; keep the previous build's UF2s as `--recover` images). Known quirks:
Rain effect params revert to compiled defaults on every flash (only the
active effect's params persist) — `just apply` restores; a flash never
reliably replaces persisted runtime state, so always `just diff`/`just
apply` from glove80-config afterwards.

## The phases

### Phase A — land this branch's work on the real topic

1. Finish verification here: run `.github/ci/test.sh`'s feature matrix (or
   equivalent nextest invocations), clippy, and the repo's no_std checks;
   fix fallout with additional commits.
2. **Decide phase C's rmk-side payload needs FIRST** (digests in the
   `GetLightingReplicaStatus` reply — almost certainly yes: add optional
   per-domain digest fields to the replica-status payload now, additive,
   snapshot-tested) so fork-fold runs ONCE.
3. Cherry-pick/rebase the real commits (NOT this handoff commit, NOT this
   directory) onto `glove80-rmk/lighting-v2` and push that branch
   (fast-forward; verify with `git ls-remote` first; never force-push).
4. In glove80-rmk's `dependencies/rmk-assembly` (fetch its origin/master
   first — it moved recently for pointing-config topics): `fork-fold update`
   for the PR-1031 entry, `fork-fold build`, resolve conflicts per that
   repo's AGENTS.md (tracked rerere/resolutions/patches; expect friction
   where pointing topics touched shared files). Push rebuilt `assembled`;
   commit+push manifest.lock/resolutions in rmk-assembly; repin BOTH
   submodules in glove80-rmk in one commit; build both glove80 halves AND
   go60 before pushing the repin.

### Phase B — glove80-rmk board side + CLI (crates only, on master)

1. Additive split-app tags in `crates/glove80-rmk/src/split_lighting.rs`
   (do NOT bump the snapshot wire VERSION): frame request
   (central→peripheral); chunked frame response and replica-status response
   (peripheral→central over `SPLIT_APP_PERIPH_TX`, depth 2 — responses must
   tolerate drops; requester times out ~300 ms and reports unavailable).
   Frame: 40 LEDs × 3 bytes, chunked into ≤26-byte payloads with an epoch
   byte so stale chunk sets never assemble.
2. Peripheral answers frames from its engine's committed frame (this
   branch's ReadFrame command) and status from `PeripheralReplication`'s
   `last_applied_revision` + `PERIPHERAL_CONTEXT` + engine state.
3. Central implements this branch's ports over that transport; expose
   `CentralReplication`'s machine state (last_acked_revision, awaiting_ack,
   generation, link_up) via a static it maintains; wire ports into
   `rynk_controller()`.
4. CLI in `crates/glove80-control`: `lighting frame [--right] [--json]`
   (per-LED RGB grid with key labels from the topology) and
   `lighting replica-status [--json]` (central vs peripheral revision,
   layer bits, powered/wake, batteries, digests, one-line verdict:
   IN SYNC / STALE(age) / UNAVAILABLE).

### Phase C — digest attestation (glove80-rmk only; design is fixed)

FNV-1a-32 digests per the binding rules above, computed by the same
`walk_snapshot` that sends; enrich the peripheral's Ack with digests; add a
~10 s unprompted attestation packet; central verifies and resnapshots on
mismatch with backoff (two consecutive post-resync mismatches → stop and
surface via replica-status, never spin); attestation-first reconnect (the
peripheral leads with what it has; matching digests suppress the reconnect
burst). Surface digests through the phase-A payload fields.

### Phase D — flash + cartesian verification (hardware on jay-lenovo)

Build via glove80-config's `just firmware` (clean tree → clean stamp).
Flash left then right with `bin/glove80-safe-flash` + `--recover`. Then the
matrix, all host-driven, recording `lighting frame` (both halves) +
`replica-status` at each point: {layer 2 active via `keymap default set 2`
vs 0} × {output_mode always-on/always-off/powered-only via config apply} ×
{link bounce if inducible}. Assert: gauge LEDs (35–39 left, 75–79 right)
lit iff layer 2 active; blue iff that half charging (left on USB reads as
charging under the VBUS proxy); replica-status IN SYNC; digests match.
Restore `keymap default set 0`, `just diff` clean, report the matrix.

## Ground rules and gotchas

- Toolchain: every cargo command through `nix develop path:.` from the
  relevant repo root. (jay-lenovo's rustup stable is broken; do the same on
  ryzen-shine for reproducibility — the dev shells are cached there.)
- Never commit to `assembled`; never force-push `glove80-rmk/lighting-v2`;
  keep commits in glove80-rmk's existing voice (`git log --oneline -15`).
- glove80-config `.claude-session/` is gitignored scratch — the copies of
  its documents in THIS directory are the durable ones.
- The go60 crate compiles the shared lighting modules — build it too or CI
  breaks (this bit us once already).
- Delete this branch (and this directory with it) once phases A–C have
  landed; nothing here is load-bearing afterwards.
