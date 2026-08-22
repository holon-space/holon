(ns probe-tail-builtin
  "Does a built-in marker that lives only in the UNFLUSHED TAIL count?
   And its mirror: does a tail-RETRACTED marker stop counting?"
  (:require [datascript.core :as d]
            [logseq.db.common.sqlite-cli :as sqlite-cli]
            [logseq.outliner.validate :as outliner-validate]
            [nbb.core :as nbb]))

(defn verdict [db e] (outliner-validate/built-in-entity? (d/entity db e)))

(defn -main [args]
  (let [[path] args
        {:keys [sqlite conn]} (sqlite-cli/open-sqlite-datascript! path)
        rows (fn [] (-> (.all ^object (.prepare sqlite "select count(*) as n from kvs"))
                        (aget 0) (aget "n")))
        tail (fn [] (let [c (-> (.all ^object (.prepare sqlite "select content from kvs where addr=1"))
                                (aget 0) (aget "content"))]
                      (count c)))]
    ;; 195 is a user block; 61 is a flushed built-in.
    (println "BEFORE  e195 built-in?" (verdict @conn 195) " e61 built-in?" (verdict @conn 61))
    (println "        rows" (rows) " tail bytes" (tail))

    (d/transact! conn [{:db/id 195 :logseq.property/built-in? true}])
    (println "\nASSERT built-in? on 195, no forced store:")
    (println "  e195 built-in?" (verdict @conn 195) " rows" (rows) " tail bytes" (tail))

    (d/transact! conn [[:db/retract 61 :logseq.property/built-in? true]])
    (println "\nRETRACT built-in? on 61, no forced store:")
    (println "  e61 built-in?" (verdict @conn 61)
             " has flag?" (pr-str (:logseq.property/built-in? (d/entity @conn 61)))
             " ident" (pr-str (:db/ident (d/entity @conn 61)))
             " file" (pr-str (:file/path (d/entity @conn 61))))
    (println "  rows" (rows) " tail bytes" (tail))))

(when (= nbb/*file* (nbb/invoked-file)) (-main *command-line-args*))
