(ns probe-mirror
  (:require [datascript.core :as d]
            [logseq.db.common.sqlite-cli :as sqlite-cli]
            [logseq.outliner.validate :as outliner-validate]
            [nbb.core :as nbb]))
(defn -main [args]
  (let [{:keys [sqlite conn]} (sqlite-cli/open-sqlite-datascript! (first args))
        v (fn [e] (outliner-validate/built-in-entity? (d/entity @conn e)))
        tail (fn [] (count (-> (.all ^object (.prepare sqlite "select content from kvs where addr=1"))
                               (aget 0) (aget "content"))))]
    (println "e40 built-in? BEFORE:" (v 40) " tail bytes" (tail))
    (d/transact! conn [[:db/retract 40 :logseq.property/built-in? true]])
    (println "e40 built-in? AFTER tail-retract:" (v 40)
             " flag now" (pr-str (:logseq.property/built-in? (d/entity @conn 40)))
             " tail bytes" (tail))))
(when (= nbb/*file* (nbb/invoked-file)) (-main *command-line-args*))
