(ns apply-edits
  "Apply a caller-supplied list of :block/title edits to a graph, through
   LogSeq's OWN transactor and storage layer.

   Exists so a head-to-head against Holon compares like with like: the caller
   picks the entities and the titles, so both sides make the identical edits in
   the identical order and any difference in the resulting file is a
   difference in the WRITER.

   $ nbb-logseq script/apply_edits.cljs GRAPH edits.json
   edits.json = [[entity-id, \"new title\"], ...]"
  (:require ["fs" :as fs]
            [cljs-bean.core :as bean]
            [datascript.core :as d]
            [datascript.storage :as storage]
            [logseq.db.common.sqlite-cli :as sqlite-cli]
            [nbb.core :as nbb]))

(defn -main [args]
  (let [[path edits-file force-store] args
        edits (js->clj (js/JSON.parse (fs/readFileSync edits-file "utf8")))
        {:keys [sqlite conn]} (sqlite-cli/open-sqlite-datascript! path)]
    (doseq [[e title] edits]
      (d/transact! conn [{:db/id e :block/title title}]))
    ;; Force a store so the TREES are written at an arbitrary N, not only when
    ;; the tail happens to overflow. Needed to bisect where two writers'
    ;; partitions first diverge.
    (when force-store
      (storage/store @conn))
    (let [row (first (bean/->clj
                      (.all ^object (.prepare sqlite "select count(*) as n from kvs"))))
          tail (first (bean/->clj
                       (.all ^object (.prepare sqlite "select content from kvs where addr = 1"))))]
      (println "applied" (count edits) "edits; rows =" (:n row))
      (println "tail after:" (subs (:content tail) 0 (min 60 (count (:content tail))))))))

(when (= nbb/*file* (nbb/invoked-file))
  (-main *command-line-args*))
