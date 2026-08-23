(ns apply-tag-edits
  "Apply a caller-supplied list of `:block/tags` edits to a graph, through
   LogSeq's OWN transactor and storage layer.

   The tag counterpart of `apply_edits.cljs`, and it exists for the same
   reason: the caller picks the entities, the tags and the order, so both
   writers make the identical edits and any difference in the resulting file
   is a difference in the WRITER.

   ONE transaction per edit, matching how Holon's push groups a block's tag
   datoms — with one datom per edit the two groupings coincide, so the
   comparison cannot be confounded by a grouping choice neither side measured.

   $ nbb-logseq script/apply_tag_edits.cljs GRAPH edits.json
   edits.json = [[block-entity, \"add\"|\"retract\", tag-entity], ...]"
  (:require ["fs" :as fs]
            [cljs-bean.core :as bean]
            [datascript.core :as d]
            [logseq.db.common.sqlite-cli :as sqlite-cli]
            [nbb.core :as nbb]))

(defn -main [args]
  (let [[path edits-file] args
        edits (js->clj (js/JSON.parse (fs/readFileSync edits-file "utf8")))
        {:keys [sqlite conn]} (sqlite-cli/open-sqlite-datascript! path)]
    (doseq [[e op tag] edits]
      (d/transact! conn [[(if (= op "add") :db/add :db/retract) e :block/tags tag]]))
    (let [row (first (bean/->clj (.all ^object (.prepare sqlite "select count(*) as n from kvs"))))
          tail (first (bean/->clj (.all ^object (.prepare sqlite "select content from kvs where addr = 1"))))]
      (println "applied" (count edits) "tag edits; rows =" (:n row))
      (println "tail after:" (subs (:content tail) 0 (min 60 (count (:content tail))))))))

(when (= nbb/*file* (nbb/invoked-file))
  (-main *command-line-args*))
