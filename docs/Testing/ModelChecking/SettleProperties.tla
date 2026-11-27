--------------------------- MODULE SettleProperties ---------------------------
(***************************************************************************)
(* The user-level properties every settle spec checks, stated once.        *)
(* A spec instantiates this module with its own expressions:               *)
(*   newest   - the version of the newest edit the user made               *)
(*   output   - the version the reader sees (the file on disk, the frame)  *)
(*   settling - a settle has started and not yet returned                  *)
(*   returned - a settle has returned and the test now reads the output    *)
(***************************************************************************)
EXTENDS Naturals
CONSTANT Writes
VARIABLES newest, output, settling, returned

\* After a settle returns, the reader sees the newest render.
SettledReadsNewest == returned => output = newest

\* Every render the user's edits owe reaches the reader.
EveryOwedRenderWritten == \A v \in 1..Writes : [](newest >= v => <>(output >= v))

\* A settle that starts returns: a test never hangs on it.
SettleReturns == [](settling => <>returned)
================================================================================
