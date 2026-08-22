(ns probe-panels
  "What ARE the non-built-in entities that hang under a built-in parent?"
  (:require [datascript.core :as d]
            [logseq.db.common.sqlite-cli :as sqlite-cli]
            [logseq.outliner.validate :as outliner-validate]
            [nbb.core :as nbb]))
(defn -main [args]
  (let [{:keys [conn]} (sqlite-cli/open-sqlite-datascript! (first args))
        db @conn
        bi? (fn [e] (outliner-validate/built-in-entity? (d/entity db e)))
        show (fn [e]
               (let [ent (d/entity db e)]
                 (println "  e=" e
                          " built-in=" (boolean (bi? e))
                          " title=" (pr-str (:block/title ent))
                          " name=" (pr-str (:block/name ent))
                          " ident=" (pr-str (:db/ident ent))
                          " parent=" (:db/id (:block/parent ent))
                          " page=" (:db/id (:block/page ent))
                          " order=" (pr-str (:block/order ent))
                          " tags=" (pr-str (mapv :db/id (:block/tags ent)))
                          " created=" (pr-str (:block/created-at ent)))))]
    (println "PARENT 188:")
    (show 188)
    (println "  188 is a PAGE?" (some? (:block/name (d/entity db 188))))
    (println "\nTHE FOUR PANELS:")
    (doseq [e [198 208 212 215]] (show e))
    (println "\nCHILDREN OF EACH PANEL (does user content hang under them?):")
    (doseq [e [198 208 212 215]]
      (let [kids (map :e (d/datoms db :avet :block/parent e))]
        (println "  panel" e "children:" (pr-str (vec kids)))
        (doseq [k kids] (show k))))
    (println "\nCHILDREN OF 188 (all):")
    (let [kids (map :e (d/datoms db :avet :block/parent 188))]
      (doseq [k kids] (show k)))
    (println "\nDo any OTHER non-built-in entities have a built-in ancestor deeper than 1?")
    (doseq [e (sort (distinct (map :e (d/datoms db :eavt))))
            :when (not (bi? e))]
      (let [p (:db/id (:block/parent (d/entity db e)))]
        (when (and p (bi? p) (not (#{198 208 212 215} e)))
          (println "  e=" e "parent=" p))))))
(when (= nbb/*file* (nbb/invoked-file)) (-main *command-line-args*))
