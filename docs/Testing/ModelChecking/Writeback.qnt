// Org write-back of one block in one document, and a test that settles and
// then reads the file: the pre-fix settle of b452395b only. The actions,
// their code anchors and the modelling choices are those of Writeback.tla.
module Writeback {
  pure val WRITES = 3
  pure val BULK = true

  type Msg = { k: str, v: int }

  var newest: int
  var store: int
  var feed: int
  var srcPending: bool
  var folding: bool
  var foldVer: int
  var supChan: List[int]
  var queued: int
  var rrChan: List[Msg]
  var bulk: bool
  var authority: int
  var base: int
  var disk: int
  var changed: int
  var processed: int
  var polled: int
  var tick: int
  var phase: str
  var snap: (int, int, int)

  // convergence.rs:converge_signals at b452395b
  val read = (store, feed, tick)
  val quiet = not(folding) and not(bulk)

  action init = all {
    newest' = 0, store' = 0, feed' = 0,
    srcPending' = false, folding' = false, foldVer' = 0,
    supChan' = [], queued' = 0, rrChan' = [], bulk' = false,
    authority' = 0, base' = 0, disk' = 0,
    changed' = 0, processed' = 0, polled' = 0, tick' = 0,
    phase' = "idle", snap' = (0, 0, 0),
  }

  action keepPipeline = all {
    feed' = feed, srcPending' = srcPending, folding' = folding,
    foldVer' = foldVer, supChan' = supChan, queued' = queued,
    rrChan' = rrChan, bulk' = bulk, authority' = authority, base' = base,
    disk' = disk, changed' = changed, processed' = processed,
    polled' = polled, tick' = tick,
  }

  action editorWrite = all {
    phase == "idle", newest < WRITES,
    newest' = newest + 1, store' = newest + 1,
    keepPipeline, phase' = phase, snap' = snap,
  }

  action feedApply = all {
    feed < store,
    feed' = store, srcPending' = true,
    newest' = newest, store' = store, folding' = folding, foldVer' = foldVer,
    supChan' = supChan, queued' = queued, rrChan' = rrChan, bulk' = bulk,
    authority' = authority, base' = base, disk' = disk, changed' = changed,
    processed' = processed, polled' = polled, tick' = tick,
    phase' = phase, snap' = snap,
  }

  action foldTake = all {
    srcPending, not(folding),
    srcPending' = false, folding' = true, foldVer' = feed,
    newest' = newest, store' = store, feed' = feed, supChan' = supChan,
    queued' = queued, rrChan' = rrChan, bulk' = bulk, authority' = authority,
    base' = base, disk' = disk, changed' = changed, processed' = processed,
    polled' = polled, tick' = tick, phase' = phase, snap' = snap,
  }

  action foldHand = all {
    folding,
    supChan' = supChan.append(foldVer), folding' = false,
    newest' = newest, store' = store, feed' = feed, srcPending' = srcPending,
    foldVer' = foldVer, queued' = queued, rrChan' = rrChan, bulk' = bulk,
    authority' = authority, base' = base, disk' = disk, changed' = changed,
    processed' = processed, polled' = polled, tick' = tick,
    phase' = phase, snap' = snap,
  }

  action consumerRoute = all {
    supChan.length() > 0,
    nondet kind = (if (BULK) Set("block", "all") else Set("block")).oneOf()
    rrChan' = rrChan.append({ k: kind, v: supChan.head() }),
    supChan' = supChan.tail(), queued' = queued + 1,
    newest' = newest, store' = store, feed' = feed, srcPending' = srcPending,
    folding' = folding, foldVer' = foldVer, bulk' = bulk,
    authority' = authority, base' = base, disk' = disk, changed' = changed,
    processed' = processed, polled' = polled, tick' = tick,
    phase' = phase, snap' = snap,
  }

  pure def lastBlock(s: List[Msg]): int =
    s.foldl(0, (acc, m) => if (m.k == "block") m.v else acc)

  action loopRerender = all {
    rrChan.length() > 0,
    authority' = if (lastBlock(rrChan) > 0) lastBlock(rrChan) else authority,
    bulk' = (bulk or rrChan.select(m => m.k == "all").length() > 0),
    queued' = queued - rrChan.length(), rrChan' = [], tick' = tick + 1,
    newest' = newest, store' = store, feed' = feed, srcPending' = srcPending,
    folding' = folding, foldVer' = foldVer, supChan' = supChan,
    base' = base, disk' = disk, changed' = changed, processed' = processed,
    polled' = polled, phase' = phase, snap' = snap,
  }

  action flushTick = all {
    bulk,
    bulk' = false, authority' = feed, tick' = tick + 1,
    newest' = newest, store' = store, feed' = feed, srcPending' = srcPending,
    folding' = folding, foldVer' = foldVer, supChan' = supChan,
    queued' = queued, rrChan' = rrChan, base' = base, disk' = disk,
    changed' = changed, processed' = processed, polled' = polled,
    phase' = phase, snap' = snap,
  }

  action netWrite = all {
    authority != base,
    disk' = authority, base' = authority, changed' = changed + 1,
    tick' = tick + 1,
    newest' = newest, store' = store, feed' = feed, srcPending' = srcPending,
    folding' = folding, foldVer' = foldVer, supChan' = supChan,
    queued' = queued, rrChan' = rrChan, bulk' = bulk, authority' = authority,
    processed' = processed, polled' = polled, phase' = phase, snap' = snap,
  }

  action fileEvent = all {
    processed < changed,
    processed' = processed + 1,
    store' = if (disk != base) disk else store,
    tick' = tick + 1,
    newest' = newest, feed' = feed, srcPending' = srcPending,
    folding' = folding, foldVer' = foldVer, supChan' = supChan,
    queued' = queued, rrChan' = rrChan, bulk' = bulk, authority' = authority,
    base' = base, disk' = disk, changed' = changed, polled' = polled,
    phase' = phase, snap' = snap,
  }

  action pollPass = all {
    polled < processed,
    polled' = processed,
    store' = if (disk != base) disk else store,
    newest' = newest, feed' = feed, srcPending' = srcPending,
    folding' = folding, foldVer' = foldVer, supChan' = supChan,
    queued' = queued, rrChan' = rrChan, bulk' = bulk, authority' = authority,
    base' = base, disk' = disk, changed' = changed, processed' = processed,
    tick' = tick, phase' = phase, snap' = snap,
  }

  action settleStart = all {
    phase == "idle",
    phase' = "poll", snap' = read,
    newest' = newest, store' = store, keepPipeline,
  }

  action settlePoll = all {
    phase == "poll",
    if (quiet and read == snap) all { phase' = "returned", snap' = snap }
    else all { phase' = phase, snap' = read },
    newest' = newest, store' = store, keepPipeline,
  }

  action testRead = all {
    phase == "returned",
    phase' = "idle", snap' = snap,
    newest' = newest, store' = store, keepPipeline,
  }

  action step = any {
    editorWrite, feedApply, foldTake, foldHand, consumerRoute, loopRerender,
    flushTick, netWrite, fileEvent, pollPass, settleStart, settlePoll,
    testRead,
  }

  // SettleProperties.tla: SettledReadsNewest
  val settledReadsNewest = phase == "returned" implies disk == newest
}
