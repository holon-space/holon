(ns probe-tag-edits
  "What a `:block/tags` edit IS, measured through LogSeq's own transactor and
   storage layer, so W3 Inc 1 is specified rather than guessed.

   `outliner-core/save-block` — the layer LogSeq's tag semantics actually live
   in — is asked FIRST and is UNREACHABLE here: it calls `(merge entity
   block)` and nbb's bundled DataScript entity is not conj-able. The probe
   records that verdict instead of working around it, because the workaround
   would be measuring something other than LogSeq.

   $ NODE_PATH=../graph-parser/node_modules nbb-logseq \\
       -cp src:../outliner/src:../graph-parser/src:../common/src \\
       script/probe_tag_edits.cljs GRAPH"
  (:require [cljs-bean.core :as bean]
            [datascript.core :as d]
            [logseq.db.common.sqlite-cli :as sqlite-cli]
            [logseq.outliner.core :as outliner-core]
            [nbb.core :as nbb]))

(defn- tail-of [^js sqlite]
  (:content (first (bean/->clj (.all ^object (.prepare sqlite "select content from kvs where addr = 1"))))))

(defn- describe [db e]
  (into (sorted-map)
        (map (fn [d] [(:a d) (:v d)]))
        (filter #(#{:block/title :block/tags :block/updated-at :block/refs :block/created-at} (:a %))
                (d/datoms db :eavt e))))

(defn -main [args]
  (let [[path] args
        {:keys [sqlite conn]} (sqlite-cli/open-sqlite-datascript! path)
        db @conn
        ;; A class entity to use as the tag, and a user block that does not
        ;; already carry it.
        task (d/entity db :logseq.class/Task)
        subject (first (remove (fn [e]
                                 (or (nil? (:block/title e))
                                     (some #(= (:db/id task) (:db/id %)) (:block/tags e))
                                     (:logseq.property/built-in? e)
                                     (:file/path e)))
                               (map #(d/entity db (:e %))
                                    (filter #(= :block/title (:a %)) (d/datoms db :eavt)))))]
    (println "tag entity" (:db/id task) (:db/ident task) (:block/title task))
    (println "subject" (:db/id subject) (pr-str (:block/title subject)))
    (println "subject before:" (pr-str (describe db (:db/id subject))))
    (println "tail before:" (tail-of sqlite))

    (println)
    (println "=== LogSeq's own outliner is NOT reachable here ===")
    (println "save-block verdict:"
             (try
               (outliner-core/save-block db {:db/id (:db/id subject) :block/tags [task]} {})
               "ACCEPTED"
               (catch :default error (str "UNREACHABLE: " (ex-message error)))))

    (println)
    (println "=== transactor level: add one tag ===")
    (d/transact! conn [[:db/add (:db/id subject) :block/tags (:db/id task)]])
    (println "subject after:" (pr-str (describe @conn (:db/id subject))))
    (println "tail after:" (tail-of sqlite))

    (println)
    (println "=== transactor level: retract that tag ===")
    (d/transact! conn [[:db/retract (:db/id subject) :block/tags (:db/id task)]])
    (println "subject after:" (pr-str (describe @conn (:db/id subject))))
    (println "tail after:" (tail-of sqlite))

    (println)
    (println "=== transactor level: retract a tag the block does NOT carry ===")
    (d/transact! conn [[:db/retract (:db/id subject) :block/tags (:db/id task)]])
    (println "tail after:" (tail-of sqlite))

    (println)
    (println "=== transactor level: a tag whose entity does not exist ===")
    (println "verdict:"
             (try
               (d/transact! conn [[:db/add (:db/id subject) :block/tags 987654]])
               (str "ACCEPTED, tags now "
                    (pr-str (map :db/id (:block/tags (d/entity @conn (:db/id subject))))))
               (catch :default error (str "REFUSED: " (ex-message error)))))
    (println "tail after:" (tail-of sqlite))))

(when (= nbb/*file* (nbb/invoked-file))
  (-main *command-line-args*))
