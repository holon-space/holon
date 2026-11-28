------------------------------ MODULE Keystroke ------------------------------
(***************************************************************************)
(* Typing into a focused SqlOnly editor (no Loro cell): keystrokes,        *)
(* dispatch, the store, data-sync echoes, a panel that shows the block,    *)
(* blur, a structural op and one external write.                           *)
(*                                                                         *)
(* A text is a sequence of key ids. Key k appends k; a key in Spaces is a  *)
(* whitespace key, which the store trims when it is trailing. The          *)
(* external writer appends ExtChar. Every action is one atomic step.       *)
(*                                                                         *)
(* The design constants select a shape of the code:                       *)
(*   Dispatch     "fifo": writes land in admission order (admit at the    *)
(*                call); "unordered": any in-flight write lands first      *)
(*   Echo         "base": evaluate_data_sync_echo; "ge": converge iff     *)
(*                echo_seq >= last_local_seq, no AdoptBaseline             *)
(*   EditorEcho   "delivered": the focused editor's data subscription     *)
(*                receives every row change; "none": it receives none     *)
(*   SeqColumn    the row the editor subscribes to carries write_seq      *)
(*   Chord        "sink": chord_commit_intent; "pending": the chord       *)
(*                flushes through pending_commit_intent                   *)
(*   SinkBaseline the keystroke sink re-baselines the commit tracking     *)
(*   Born         "present": the block exists; "admitted": the first      *)
(*                keystroke births it, the create admitted first;         *)
(*                "detached": the create runs beside the keystroke        *)
(*   Strip        "defer": a slash command's strip runs before the next   *)
(*                key; "spawn": it runs on a later tick from a snapshot   *)
(*   Write        "whole": a keystroke writes the whole buffer;           *)
(*                "delta": it appends its key to the stored text          *)
(*   Trim         the store trims trailing whitespace                     *)
(* The world constants: Keys, Spaces, Struct ("none", "chord", "split",   *)
(* "join"), External ("none", "sticky": no write_seq, "fresh": a new one), *)
(* ExtWhen ("any", "after": only once every key is typed), Blur.          *)
(* Disable names one action that a teeth run switches off.                 *)
(***************************************************************************)
EXTENDS Integers, Sequences, FiniteSets

CONSTANTS Keys, Spaces, Struct, External, ExtWhen, Blur,
          Dispatch, Echo, EditorEcho, SeqColumn, Chord, SinkBaseline, Born,
          Strip, Write, Trim, Disable

ExtChar == 9
NoSeq == -1
Blocks == {1, 2}

ASSUME /\ Keys \in 1..8 /\ Spaces \subseteq 1..Keys
       /\ Struct \in {"none", "chord", "split", "join"}
       /\ External \in {"none", "sticky", "fresh"} /\ ExtWhen \in {"any", "after"}
       /\ Blur \in BOOLEAN /\ Dispatch \in {"fifo", "unordered"}
       /\ Echo \in {"base", "ge"} /\ EditorEcho \in {"delivered", "none"}
       /\ SeqColumn \in BOOLEAN /\ Chord \in {"sink", "pending"}
       /\ SinkBaseline \in BOOLEAN /\ Born \in {"present", "admitted", "detached"}
       /\ Strip \in {"defer", "spawn"} /\ Write \in {"whole", "delta"} /\ Trim \in BOOLEAN
       /\ Disable \in {"none", "Apply", "Converge"}

VARIABLES
    nk,        \* keys typed so far (key ids 1..nk)
    home,      \* the block each typed key belongs to
    stopped,   \* the user has stopped acting
    focus,     \* the block the editor edits
    mounted,   \* an editor is mounted on focus
    buf,       \* EditorViewModel::buffer
    base,      \* the commit funnels' baseline (ViewEventHandler::set_baseline)
    last,      \* EditorViewModel::last_local_seq
    clock,     \* write_seq::COUNTER; also stamps writes that carry no seq
    queue,     \* admitted, not yet applied writes
    create,    \* a detached create is in flight
    born,      \* the first keystroke asked for the block's birth
    ex,        \* the block's row exists
    sc,        \* block_raw.content
    ss,        \* block_raw.write_seq
    st,        \* stamp of the composition the stored content came from
    ech,       \* row changes in transit to the editor's data subscription
    pch,       \* row changes in transit to the panel
    pv,        \* what the panel shows: [c |-> content, t |-> stamp]
    extDone,   \* the external write was admitted
    extHome,   \* the block that holds the external text
    dec,       \* the last echo decision
    sop,       \* the structural op: "none", "pending", "done", "aborted"
    moved,     \* focus followed the structural op
    snap       \* a slash command's pending strip: [on, c]

vars == <<nk, home, stopped, focus, mounted, buf, base, last, clock, queue,
          create, born, ex, sc, ss, st, ech, pch, pv, extDone, extHome, dec,
          sop, moved, snap>>

Range(s) == {s[i] : i \in DOMAIN s}
Max(a, b) == IF a > b THEN a ELSE b
RECURSIVE Canon(_)
\* Models: crates/holon-api/src/content_canonical.rs:canonicalize_stored_content
\* (text blocks of one line: trailing whitespace trimmed; no text here starts
\* with whitespace, so the leading trim never applies).
Canon(s) == IF Trim /\ s # <<>> /\ s[Len(s)] \in Spaces THEN Canon(SubSeq(s, 1, Len(s) - 1)) ELSE s
RowSeq(s) == IF SeqColumn THEN s ELSE NoSeq
Drop(s, i) == [j \in 1..(Len(s) - 1) |-> IF j < i THEN s[j] ELSE s[j + 1]]

\* Models: crates/holon-frontend/src/echo.rs:evaluate_data_sync_echo
Decide(cur, new, s, l) ==
    IF cur = new THEN "InSync"
    ELSE IF s = NoSeq THEN "DropNoSeq"
    ELSE IF s < l THEN "DropStale"
    ELSE IF Echo = "base" /\ s = l /\ Canon(cur) = new THEN "AdoptBaseline"
    ELSE "Converge"

Init ==
    /\ nk = 0 /\ home = [k \in 1..Keys |-> 0] /\ stopped = FALSE
    /\ ex = [b \in Blocks |-> IF b = 1 THEN Born = "present" ELSE Struct = "join"]
    /\ sc = [b \in Blocks |-> <<>>] /\ ss = [b \in Blocks |-> 0] /\ st = [b \in Blocks |-> 0]
    /\ focus = IF Struct = "join" THEN 2 ELSE 1
    /\ mounted = TRUE /\ buf = <<>> /\ base = <<>> /\ last = 0 /\ clock = 0
    /\ queue = <<>> /\ create = FALSE /\ born = FALSE
    \* the subscription's first emission is the row as mounted
    /\ ech = IF ex[focus] /\ EditorEcho = "delivered" THEN <<[c |-> <<>>, s |-> RowSeq(0)]>> ELSE <<>>
    /\ pch = [b \in Blocks |-> <<>>] /\ pv = [b \in Blocks |-> [c |-> <<>>, t |-> 0]]
    /\ extDone = FALSE /\ extHome = focus /\ dec = "none"
    /\ sop = "none" /\ moved = FALSE /\ snap = [on |-> FALSE, c |-> <<>>]

-----------------------------------------------------------------------------
(* The editor *)

\* A local edit of the buffer to text new (a keystroke or a programmatic
\* set_value that fires the change event).
\* Models: crates/holon-frontend/src/editor_view_model.rs:EditorViewModel::apply_local_edit
\* (SqlOnly arm: write_seq::next, last_local_seq, set_baseline, the set_field
\* intent) and frontends/gpui/src/views/editor_view.rs:EditorView::new (the
\* InputEvent::Change arm dispatches the intent at once; dispatch_intent
\* admits it at the call: crates/holon-frontend/src/reactive.rs:dispatch_intent,
\* crates/holon-api/src/operation_engine.rs:OperationEngine::admit).
LocalEdit(new, key) ==
    LET seq == clock + 1
        birth == IF ~ex[focus] /\ ~born /\ Born = "admitted"
                 THEN <<[k |-> "create", b |-> focus, c |-> <<>>, s |-> 0, t |-> 0, key |-> 0]>>
                 ELSE <<>>
    IN /\ buf' = new
       /\ clock' = seq
       /\ last' = seq
       /\ base' = IF SinkBaseline THEN new ELSE base
       /\ queue' = queue \o birth \o <<[k |-> "key", b |-> focus, c |-> new, s |-> seq, t |-> seq, key |-> key]>>
       /\ born' = (born \/ ~ex[focus])
       \* Models: crates/holon-frontend/src/reactive.rs:birth_creation_affordance
       \* ("detached": the create of the pre-fix shape, spawned beside the key).
       /\ create' = (create \/ (~ex[focus] /\ ~born /\ Born = "detached"))

Type ==
    /\ ~stopped /\ mounted /\ nk < Keys
    /\ nk' = nk + 1
    /\ home' = [home EXCEPT ![nk + 1] = focus]
    /\ LocalEdit(Append(buf, nk + 1), nk + 1)
    /\ UNCHANGED <<stopped, focus, mounted, ex, sc, ss, st, ech, pch, pv, extDone, extHome,
                   dec, sop, moved, snap>>

\* One row change reaches the editor. The subscription is a deduped signal:
\* it may skip to a later change, never back.
\* Models: frontends/gpui/src/views/editor_view.rs:EditorView::new (the data-sync
\* subscription) and
\* crates/holon-frontend/src/editor_view_model.rs:EditorViewModel::converge_from_data_sync,
\* with EditorViewModel::set_buffer_from_authority on Converge.
DeliverEcho ==
    \E i \in 1..Len(ech) :
        LET e == ech[i]
            d == Decide(buf, e.c, e.s, last)
            adopt == d = "Converge" /\ Disable # "Converge"
        IN /\ ech' = SubSeq(ech, i + 1, Len(ech))
           /\ dec' = d
           /\ buf' = IF adopt THEN e.c ELSE buf
           /\ base' = IF adopt \/ d = "AdoptBaseline" THEN e.c ELSE base
           /\ last' = IF d \in {"InSync", "AdoptBaseline"} \/ adopt THEN Max(last, e.s) ELSE last
           /\ UNCHANGED <<nk, home, stopped, focus, mounted, clock, queue, create, born, ex,
                          sc, ss, st, pch, pv, extDone, extHome, sop, moved, snap>>

\* A focus leave: the pending text goes to the store before the editor goes.
\* Models: crates/holon-frontend/src/editor_view_model.rs:EditorViewModel::on_blur
FlushOf(b) ==
    IF buf # base
    THEN <<[k |-> "flush", b |-> b, c |-> buf, s |-> NoSeq, t |-> clock + 1, key |-> 0]>>
    ELSE <<>>

BlurAway ==
    /\ Blur /\ ~stopped /\ mounted /\ Struct \notin {"split", "join"}
    /\ queue' = queue \o FlushOf(focus)
    /\ clock' = clock + 1
    /\ mounted' = FALSE
    /\ ech' = <<>>
    /\ UNCHANGED <<nk, home, stopped, focus, buf, base, last, create, born, ex, sc, ss, st,
                   pch, pv, extDone, extHome, dec, sop, moved, snap>>

\* A structural key (Tab, Enter at the end, Backspace at 0): the pending text
\* is dispatched first, then the op.
\* Models: frontends/gpui/src/views/editor_view.rs:dispatch_structural_as_commit_point
\* and crates/holon-frontend/src/editor_view_model.rs:EditorViewModel::chord_commit_intent
\* ("pending": EditorViewModel::pending_commit_intent).
Structural ==
    /\ ~stopped /\ mounted /\ sop = "none" /\ Struct # "none" /\ ex[focus]
    /\ LET flush == IF Chord = "sink" THEN <<>> ELSE FlushOf(focus)
           op == CASE Struct = "split" -> <<[k |-> "split", b |-> 1, c |-> <<>>, s |-> Len(buf), t |-> 0, key |-> 0]>>
                   [] Struct = "join" -> <<[k |-> "join", b |-> 2, c |-> <<>>, s |-> 0, t |-> 0, key |-> 0]>>
                   [] OTHER -> <<>>
       IN /\ queue' = queue \o flush \o op
          /\ sop' = IF Struct = "chord" THEN "done" ELSE "pending"
    /\ clock' = clock + 1
    /\ UNCHANGED <<nk, home, stopped, focus, mounted, buf, base, last, create, born, ex, sc,
                   ss, st, ech, pch, pv, extDone, extHome, dec, moved, snap>>

\* Focus follows a structural op that landed: the old editor commits its
\* pending text and goes; a new editor mounts on the row as stored.
\* Models: crates/holon-frontend/src/reactive.rs:apply_structural_focus and
\* crates/holon-frontend/src/editor_view_model.rs:EditorViewModel::new (the
\* seed; last_local_seq starts at WriteSeq::ZERO).
FollowFocus ==
    /\ sop = "done" /\ Struct \in {"split", "join"} /\ ~moved
    /\ LET to == IF Struct = "split" THEN 2 ELSE 1
       IN /\ focus' = to
          /\ buf' = sc[to] /\ base' = sc[to] /\ last' = 0
          /\ ech' = IF EditorEcho = "delivered" THEN <<[c |-> sc[to], s |-> RowSeq(ss[to])]>> ELSE <<>>
    /\ queue' = queue \o FlushOf(focus)
    /\ clock' = clock + 1
    /\ moved' = TRUE
    /\ UNCHANGED <<nk, home, stopped, mounted, create, born, ex, sc, ss, st, pch, pv, extDone,
                   extHome, dec, sop, snap>>

\* Enter on a slash command: the command text is stripped from the buffer.
\* Models: frontends/gpui/src/views/editor_view.rs:apply_popup_action
\* ("spawn": the strip of the pre-fix shape, applied on a later tick from the
\* text read at Enter).
SlashEnter ==
    /\ Strip = "spawn" /\ ~stopped /\ mounted /\ ~snap.on /\ nk > 0
    /\ snap' = [on |-> TRUE, c |-> buf]
    /\ UNCHANGED <<nk, home, stopped, focus, mounted, buf, base, last, clock, queue, create,
                   born, ex, sc, ss, st, ech, pch, pv, extDone, extHome, dec, sop, moved>>

ApplyStrip ==
    /\ snap.on /\ mounted
    /\ snap' = [on |-> FALSE, c |-> <<>>]
    /\ IF snap.c # buf
       THEN LocalEdit(snap.c, 0)
       ELSE UNCHANGED <<buf, clock, last, base, queue, born, create>>
    /\ UNCHANGED <<nk, home, stopped, focus, mounted, ex, sc, ss, st, ech, pch, pv, extDone,
                   extHome, dec, sop, moved>>

Stop ==
    /\ ~stopped
    /\ stopped' = TRUE
    /\ UNCHANGED <<nk, home, focus, mounted, buf, base, last, clock, queue, create, born, ex,
                   sc, ss, st, ech, pch, pv, extDone, extHome, dec, sop, moved, snap>>

-----------------------------------------------------------------------------
(* The outside world *)

\* Modelling choice: a non-editor write to the focused block (an MCP
\* set_field, an org re-ingest, a second occurrence's editor), admitted like
\* any write. "sticky" leaves write_seq as it is
\* (crates/holon/src/core/sql_operation_provider.rs:SqlOperationProvider::execute_operation_with_origin, its set_field arm
\* without a write_seq param); "fresh" stamps a new one.
ExternalWrite ==
    /\ External # "none" /\ ~extDone /\ mounted /\ ex[focus]
    /\ ExtWhen = "after" => nk = Keys
    /\ queue' = Append(queue, [k |-> "ext", b |-> focus, c |-> <<>>,
                               s |-> IF External = "fresh" THEN clock + 1 ELSE NoSeq,
                               t |-> clock + 1, key |-> 0])
    /\ clock' = clock + 1
    /\ extDone' = TRUE /\ extHome' = focus
    /\ UNCHANGED <<nk, home, stopped, focus, mounted, buf, base, last, create, born, ex, sc,
                   ss, st, ech, pch, pv, dec, sop, moved, snap>>

-----------------------------------------------------------------------------
(* The store *)

\* The row change that a write of content c and write_seq s with stamp t to
\* block b causes, and its delivery to the subscribers.
Change(b, c, s, t) ==
    LET changed == c # sc[b] \/ s # ss[b]
    IN /\ sc' = [sc EXCEPT ![b] = c]
       /\ ss' = [ss EXCEPT ![b] = s]
       /\ st' = IF changed THEN [st EXCEPT ![b] = t] ELSE st
       /\ pch' = IF changed THEN [pch EXCEPT ![b] = Append(@, [c |-> c, t |-> t])] ELSE pch
       /\ ech' = IF changed /\ mounted /\ focus = b /\ EditorEcho = "delivered"
                 THEN Append(ech, [c |-> c, s |-> RowSeq(s)]) ELSE ech

\* Models: crates/holon/src/core/sql_operation_provider.rs:SqlOperationProvider::execute_operation_with_origin
\* (its set_field arm: one UPDATE of content and, when the write carries one, write_seq; the
\* stored text is trimmed). A write to a missing row matches no row.
ApplyWrite(it) ==
    /\ ex[it.b]
    /\ CASE it.k = "key" ->
              Change(it.b, Canon(IF Write = "whole" THEN it.c ELSE Append(sc[it.b], it.key)), it.s, it.t)
         [] it.k = "flush" -> Change(it.b, Canon(it.c), ss[it.b], it.t)
         [] it.k = "ext" ->
              Change(it.b, Append(sc[it.b], ExtChar), IF it.s = NoSeq THEN ss[it.b] ELSE it.s, it.t)
    /\ UNCHANGED <<ex, home, extHome, sop>>

\* Models: crates/holon-core/src/traits.rs:BlockOperations::split_block (the tail from
\* the position on goes to a new block; a position past the stored text
\* fails the op).
ApplySplit(it) ==
    IF it.s > Len(sc[1])
    THEN /\ sop' = "aborted" /\ UNCHANGED <<ex, sc, ss, st, pch, ech, home, extHome>>
    ELSE LET tail == SubSeq(sc[1], it.s + 1, Len(sc[1]))
             head == SubSeq(sc[1], 1, it.s)
         IN /\ sop' = "done"
            /\ ex' = [ex EXCEPT ![2] = TRUE]
            /\ sc' = [sc EXCEPT ![1] = head, ![2] = tail]
            /\ ss' = [ss EXCEPT ![2] = 0]
            /\ st' = [st EXCEPT ![1] = IF tail = <<>> THEN @ ELSE clock, ![2] = clock]
            /\ pch' = [pch EXCEPT ![1] = IF tail = <<>> THEN @ ELSE Append(@, [c |-> head, t |-> clock]),
                                  ![2] = Append(@, [c |-> tail, t |-> clock])]
            /\ ech' = IF tail # <<>> /\ mounted /\ focus = 1 /\ EditorEcho = "delivered"
                      THEN Append(ech, [c |-> head, s |-> RowSeq(ss[1])]) ELSE ech
            /\ home' = [k \in 1..Keys |-> IF home[k] = 1 /\ k \in Range(tail) THEN 2 ELSE home[k]]
            /\ extHome' = IF ExtChar \in Range(tail) THEN 2 ELSE extHome

\* Models: crates/holon-core/src/traits.rs:BlockOperations::join_block (block 2's text
\* is appended to block 1, block 2 is deleted).
ApplyJoin(it) ==
    /\ sop' = "done"
    /\ ex' = [ex EXCEPT ![2] = FALSE]
    /\ sc' = [sc EXCEPT ![1] = sc[1] \o sc[2]]
    /\ st' = [st EXCEPT ![1] = IF sc[2] = <<>> THEN @ ELSE clock]
    /\ pch' = [pch EXCEPT ![1] = IF sc[2] = <<>> THEN @ ELSE Append(@, [c |-> sc[1] \o sc[2], t |-> clock])]
    /\ ech' = IF sc[2] # <<>> /\ mounted /\ focus = 1 /\ EditorEcho = "delivered"
              THEN Append(ech, [c |-> sc[1] \o sc[2], s |-> RowSeq(ss[1])]) ELSE ech
    /\ home' = [k \in 1..Keys |-> IF home[k] = 2 THEN 1 ELSE home[k]]
    /\ extHome' = 1
    /\ UNCHANGED ss

ApplyCreate(it) ==
    /\ ex' = [ex EXCEPT ![it.b] = TRUE]
    /\ ech' = IF mounted /\ focus = it.b /\ EditorEcho = "delivered"
              THEN Append(ech, [c |-> <<>>, s |-> RowSeq(0)]) ELSE ech
    /\ UNCHANGED <<sc, ss, st, pch, home, extHome, sop>>

\* One admitted write lands. "fifo": the oldest admitted one; "unordered":
\* any in flight.
\* Models: crates/holon-api/src/admission.rs (claims released in admission
\* order on overlapping footprints) and the dispatch task spawned in
\* crates/holon-frontend/src/reactive.rs:dispatch_intent.
Apply ==
    \E i \in 1..Len(queue) :
        LET it == queue[i]
        IN /\ Dispatch = "fifo" => i = 1
           /\ Disable = "Apply" => it.k # "key"
           /\ queue' = Drop(queue, i)
           /\ CASE it.k = "split" -> ApplySplit(it)
                [] it.k = "join" -> ApplyJoin(it)
                [] it.k = "create" -> ApplyCreate(it)
                [] ~ex[it.b] -> UNCHANGED <<ex, sc, ss, st, pch, ech, home, extHome, sop>>
                [] OTHER -> ApplyWrite(it)
           /\ UNCHANGED <<nk, stopped, focus, mounted, buf, base, last, clock, create, born, pv,
                          extDone, dec, moved, snap>>

\* Models: crates/holon-frontend/src/reactive.rs:birth_creation_affordance
\* (the spawned create of the pre-fix shape).
ApplyDetachedCreate ==
    /\ create
    /\ create' = FALSE
    /\ ApplyCreate([k |-> "create", b |-> focus, c |-> <<>>, s |-> 0, t |-> 0, key |-> 0])
    /\ UNCHANGED <<nk, stopped, focus, mounted, buf, base, last, clock, queue, born, pv,
                   extDone, dec, moved, snap>>

\* Modelling choice: a second view of the block (a backlink panel, a search
\* result) that renders the row as delivered; the same deduped signal.
DeliverPanel ==
    \E b \in Blocks : \E i \in 1..Len(pch[b]) :
        /\ pv' = [pv EXCEPT ![b] = pch[b][i]]
        /\ pch' = [pch EXCEPT ![b] = SubSeq(@, i + 1, Len(@))]
        /\ UNCHANGED <<nk, home, stopped, focus, mounted, buf, base, last, clock, queue, create,
                       born, ex, sc, ss, st, ech, extDone, extHome, dec, sop, moved, snap>>

Next ==
    \/ Type
    \/ DeliverEcho
    \/ BlurAway
    \/ Structural
    \/ FollowFocus
    \/ SlashEnter
    \/ ApplyStrip
    \/ Stop
    \/ ExternalWrite
    \/ Apply
    \/ ApplyDetachedCreate
    \/ DeliverPanel

\* Fairness: every system step that stays enabled happens. Apply: the
\* runtime polls every spawned dispatch task and admission releases claims in
\* order. DeliverEcho, DeliverPanel: the LiveData row signal and the views'
\* spawned loops. FollowFocus, ApplyStrip, ApplyDetachedCreate: the
\* foreground tasks that run them. The user and the external writer get none.
Spec ==
    /\ Init /\ [][Next]_vars
    /\ WF_vars(Apply) /\ WF_vars(DeliverEcho) /\ WF_vars(DeliverPanel)
    /\ WF_vars(FollowFocus) /\ WF_vars(ApplyStrip) /\ WF_vars(ApplyDetachedCreate)

-----------------------------------------------------------------------------
(* User-level properties *)

\* Every typed key is in the store and in the editor of its block; a
\* whitespace key may be trimmed from the store. Every view of a block shows
\* the store; the editor's text trims to it. A structural key took effect.
Converged ==
    /\ sop \in {"none", "done"}
    /\ \A k \in 1..nk :
          /\ k \in Spaces \/ (ex[home[k]] /\ k \in Range(sc[home[k]]))
          /\ (mounted /\ focus = home[k]) => k \in Range(buf)
    /\ \A b \in Blocks : ex[b] => pv[b].c = sc[b]
    /\ (mounted /\ ex[focus]) => Canon(buf) = sc[focus]

NoLostKeystroke == <>[]stopped => <>[]Converged

\* No write-through disclosure of a dropped external write exists.
Disclosed == FALSE

Kept == extDone => (Disclosed \/ (/\ ExtChar \in Range(sc[extHome])
                                  /\ (mounted /\ focus = extHome) => ExtChar \in Range(buf)))

ExternalWriteKept == <>[]stopped => <>[]Kept

NoClobber == [][\A b \in Blocks : st'[b] >= st[b]]_vars

MonotoneViews ==
    [][/\ \A b \in Blocks : pv'[b].t >= pv[b].t
       /\ (mounted /\ mounted' /\ focus' = focus) => (Range(buf) \cap 1..nk) \subseteq Range(buf')]_vars

-----------------------------------------------------------------------------
(* Witnesses: each must be violated *)

NotTypedAndStopped == ~(nk = Keys /\ stopped /\ Converged)
NoStaleEcho == dec # "DropStale"
NoInSync == dec # "InSync"
NoAdoptBaseline == dec # "AdoptBaseline"
NoConverge == dec # "Converge"
NoDropNoSeq == dec # "DropNoSeq"
NeverRewritten == \A b \in Blocks : ss[b] <= 1
PanelNeverMoves == \A b \in Blocks : pv[b].t = 0
ExternalNeverAdopted == ~(extDone /\ ExtChar \in Range(buf))

\* The oracles of the red tests. Once the external write landed, the store
\* keeps its text.
ExtLanded == extDone /\ \A i \in 1..Len(queue) : queue[i].k # "ext"
ExtStaysStored == ExtLanded => ExtChar \in Range(sc[extHome])
\* A typed whitespace key stays in the editor of its block.
SpaceStaysInEditor == \A k \in Spaces : (k <= nk /\ mounted /\ focus = home[k]) => k \in Range(buf)
\* A structural op the user keyed is not refused.
SplitLands == sop # "aborted"
================================================================================
