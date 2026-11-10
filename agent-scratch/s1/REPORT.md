# cont139-S1 report: sub-batch deco JNI crossings (H4)

2026-11-10T20:25:50Z START: session cont139-S1 begins; read PROGRESS.json + worklog. BUILD_OK not yet present -> analysis/code only.
Worktree: /home/z/wt_s1b branch cont139-s1 @ 0091c8ea (= origin/main, fetch made no update).

## 1. MAP OF DECO-PATH JNI CROSSING SITES (ground truth, file:line @ 0091c8ea)

Java->Rust entries (ChunkGenNative.java decls; call sites in DecoDriver.java):
- DecoDriver.runChunk:310  decoDecoSeed           -> 1/chunk
- DecoDriver.runChunk:323  decoChunkBegin         -> 1/chunk (resets chunk_ov/ov_cols/snap/hms/zoom_seed; native: features_jni.rs:1316)
- DecoDriver.runChunk:339  decoChunkPlan          -> 1/chunk (features_jni.rs:1016)
- DecoDriver.runChunk:374  decoRunFeature         -> 55.4/ch ("features"; features_jni.rs:1084). THE TARGET.
- DecoDriver.runChunk:420  decoOpsDropped         -> only when ndrop>0 (rare; features_jni.rs:1568)
- DecoDriver.runChunk:393  decoDelegateMark       -> per delegated feature (features_jni.rs:1674)
- DecoDriver.runChunk:292  decoSetFlags           -> once per world (FLAGS_SENT)
- OreHook lane oreRunConfigured (ChunkGenNative.java:211) = deco=0 path, out of scope.

Rust->Java upcalls INSIDE decoRunFeature (features_jni.rs BridgeMids:139-177, resolved
once/process): decoSnapSection/decoSnapSectionRange (lazy 3x3 section snapshot loader =
"snap_sec" 92.8/ch, DecoStats.SNAP_SECTIONS), decoHeightmaps (WG 512-int handshake,
cached per region chunk in DecoCtx.chunk_hms), height/stateIs/fluidIs/isAir/isReplaceable/
isSolid/hasSturdyFace/wouldSurvive/biomeHasFeature/stateFull/isSolidRender/stateName/tagIs/
tagMembers/freezeChecks (= "upcalls" 64.6/ch, DecoStats UP_* snapshot:156).

Java-only per chunk: applyOp x1377.5 ("ops" — not crossings), delegateVanilla, runStructures.
crossings_all 258 = 64.6 up + 55.4 feat + 92.8 snap + ~45 misc (seed/begin/plan/drops/marks/hm). ✓ consistent.

Read-path facts that make batching legal (verified in code):
- PlaceCtx.emit -> write_overlay at OP PRODUCTION time (features_place.rs:60,88): the native
  overlay carries ALL native writes since chunk begin, so ChunkLevel point queries
  (overlay->snapshot->bridge) are independent of Java apply timing.
- chunk_hms WG heightmaps are emulated update-on-write native-side (features_snap.rs:553) —
  also apply-timing independent.
- Live-region (JNI bridge) readers inside a feature: freezeChecks (place_freeze_top_layer
  features_place.rs:3119, reads region.getHeight(MOTION_BLOCKING)+writes ICE/SNOW via region)
  and the bridge FALLBACKS (unsnapshotted positions only). => flush boundaries required
  BEFORE delegates and BEFORE native freeze_top_layer; everything else batches.

## 2. S1 DESIGN (implemented this session, kill-switch CRUS_S1_SUBBATCH, default OFF)

New native entry decoRunBatch(ctx, decoSeed, paperSeed, ox, oy, oz, maxY, int[] runs,
nRuns, startRun, bridge, int[] ops, int[] perRun, int[] meta) -> opsWritten: expands
CONSECUTIVE (step,gid) runs in ONE crossing with the exact per-feature protocol
(run_feature_staged == the run_feature_master body, refactored, one seeding path), writing
all ops into the shared pinned buffer. Batch stops at boundaries:
  kind 1 DELEGATE: !placed_fully_supported (Java then runs delegateVanilla+decoDelegateMark)
  kind 2 FREEZE:   freeze_top_layer next with ops pending (flush first; freeze runs at the
                   next call start so its live-region upcalls see the flushed state = old)
  kind 3 FULL:     ops buffer would overflow (ops carry over in DecoCtx.batch_carry —
                   they are already in the overlay and MUST be applied; drained first next call)
  kind 0 END.      meta=[completedRuns, kind, opsWritten]; perRun[k]=ops of run startRun+k.
Java DecoDriver: batched loop applies per-run segments (per-run drop sync kept => no extra
crossings when healthy), per-boundary delegate handling, falls back to the exact old
per-feature loop (runPerFeature) on native failure (-1), panic skip (-2: apply prefix,
skip 1, continue batching). Rust test pins byte-identical op streams old-vs-batch over the
full 26.3 plan incl. delegate/freeze/buffer-full boundaries (ffi.rs, mod s1_batch_parity).
2026-11-10T21:0xZ IMPLEMENTATION COMPLETE (code written, NOT yet compiled — CPU gate active):

Rust (native/src):
- features_master.rs: run_feature_master refactored into run_feature_staged (shared
  seeding/placement body) + serialize_ops (exact bounds guard) — the per-feature path
  is byte-identical by construction. New: BatchBoundary{End=0,Delegate=1,Freeze=2,Full=3},
  BatchResult, is_freeze_like("minecraft:freeze_top_layer"), run_batch_core (drain carry ->
  loop runs: delegate gate -> freeze gate -> stage+serialize with tail-carry on partial
  serialize). DecoCtx.batch_carry field added (cleared in decoChunkBegin).
- features_jni.rs: Java_com_planeta_crussty_ChunkGenNative_decoRunBatch export (runs[],
  perRun[], meta[] pinned via GetPrimitiveArrayCritical; one JniLevel+ChunkLevel per call;
  cont-93 panic discipline -> -2 + valid prefix in meta; PROF_LOADER/NATIVE/RUNS + PROF_FEAT
  per-run attribution). decoChunkBegin clears batch_carry.
- ffi.rs: mod s1_batch_parity — op-stream parity old-vs-batch over the FULL embedded-26.3
  plan: production buffer (End/Delegate/Freeze boundaries) + tiny buffer sized to the max
  single feature (forces Full boundaries + carry drains, never truncation); asserts byte
  equality of both concatenated streams + identical final overlay/epoch digest.

Java (plugin/src/com/planeta/crussty):
- ChunkGenNative.java: decoRunBatch declaration + full ABI contract comment.
- DecoDriver.java: S1_SUBBATCH (static final from env CRUS_S1_SUBBATCH, default OFF) +
  S1_BATCH_BROKEN runtime guard; old loop extracted VERBATIM into runPerFeature (kill-switch
  OFF lane + fallback); runChunkBatched: batch loop with per-run segment apply (per-run drop
  sync preserved), delegate handling at kind==1 (delegateVanilla+decoDelegateMark), panic
  skip at kind==4, partial-carry-drain segment (done==0&&valid>0), stall guard, per-feature
  fallback on -1/hard-failure. applyOp instance shim removed (both lanes call applyOpStatic).
- DecoStats.java: S1_BATCHES / S1_BATCH_RUNS counters + snapshot "batch=Calls/Runs" +
  reset().

KNOWN STATS-LEVEL CAVEAT (blocks unaffected): if a feature's ops split across a buffer-full
boundary, the drained tail is attributed to the NEXT run's stats segment (perName op counts
shift by the tail; FEATURES_TOTAL still exact; op stream order preserved). Needs >65536
buffered ops in one batch = never on real packs.

Next: poll BUILD_OK -> cargo build/test (88/88 + 1 new test = 89 expected), javac plugin
build via build.sh (worktree-local out), commit, artifacts md5.
2026-11-10T21:1xZ Static review pass done (no compile yet): fixed test-closure lifetimes
(closures must outlive ChunkLevel<'a>), removed a redundant post-panic perRun write
(core writes per_run_out as runs complete -> panic-consistent), exact serialize-guard
tail handling (carry tail when written>0, old truncation semantics when written==0),
partial-drain segment apply in Java (done==0 && valid>0).

## 5. A/B INSTRUCTIONS FOR THE ORCHESTRATOR (draft — md5s appended after build)

Build prerequisites: branch cont139-s1 (worktree /home/z/wt_s1b), artifacts:
  native: /home/z/wt_s1b/native/target-release/libcrussty_chunkgen.so (CARGO_TARGET_DIR=/home/z/tgt_s1b)
  plugin: worktree-local jar (see build.sh run below — output OUT stays in the worktree)
Deploy BOTH (jar+so are a matched pair; the jar tolerates an old .so via the
S1_BATCH_BROKEN fallback, but then the A/B measures nothing).

Arm A: CRUS_S1_SUBBATCH=1  (batched lane)
Arm B: env unset           (per-feature lane = pre-cont139 behavior, same binaries)

Protocol: wipe world*, same coords (RX1=2000 RZ1=2000 honest window 144ch), back-to-back
paired windows, 3x each arm, pinned artifact md5s. Existing campaign config: worker=2,
FILL_THREADS=2, -Ddisable.watchdog=true, Q5 pool, deleg ports.

What to compare (crusgenstats native deco stage line + honest window):
1. honest e2e ms/ch (windowSnapshot span) — expected direction: -0.3..-1.5 ms/ch (H4:
   0.2-0.4ms idle wrapper mass + more loaded; 2-core park/unpark+safepoint per crossing).
2. deco run= phase (DecoStats RUN_TOTAL) — expect a DROP of ~(features/batches-1) x
   crossing overhead; run= must NOT increase (more native work per crossing is the same work).
3. features=N vs batch=Calls/Runs counters (new "batch=%d/%d" tail): crossings collapse
   check: features≈Runs should be ~unchanged (~55/ch), Calls should be ~5-12/ch.
   crossing delta expected: (features - batches) fewer decoRunFeature crossings/ch,
   plus fewer decoOpsDropped/decoDelegateMark calls (drops per batch; marks unchanged).
4. ops=N MUST be identical across arms (bit-parity canary: same seed, same window).
5. snap-sec=S/ms — expected ~unchanged (NOT in S1 scope; see honest residual below).
6. block-level judge / dptt set if the orchestrator has a lane for it (the CI judge is
   the bit-parity gate; filldump parity is untouched by this change).

Watch-fors (abort arm A if seen): "run_batch PANIC" / "S1 batch stall" / "hard failure"
log lines (each auto-falls back per-feature — the arm then measures the fallback lane,
numbers invalid for H4), and any ops= mismatch (parity bug — kill the branch).

## 4. HONESTLY NOT VERIFIED (this session, by design)
- NO live-server crossing counts (BUILD_OK gate + rule 5: no server start) — the A/B
  above is the orchestrator's job.
- NO compile yet (cargo/javac) — first build may surface mechanical errors; budgeted.
- The ~40 crossings/ch H4 figure is NOT reached by this change alone: batching collapses
  the 55.4 decoRunFeature crossings to ~#boundaries (est. 5-12/ch) + slightly fewer
  drops/marks; upcalls 64.6 and snap_sec 92.8 remain (they are per-query crossings
  INSIDE the native, not wrapper crossings — prefetching sections was evaluated and
  REJECTED: per-section Java build ~3.5-4.8us dominates the ~2us crossing (measured in
  the CRUS_SNAP_RANGE A/B notes) — bulk prefetch would trade ~85 crossings for ~+0.5ms
  Java CPU/ch). Honest projection: crossings_all 258 -> ~200-215 (-17-22%), wrapper wall
  -0.2-0.4ms idle / more loaded. The remaining mass (upcalls+snap) needs an S2 lever.
