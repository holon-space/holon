(ns probe-views
  (:require [datascript.core :as d]
            [logseq.db.common.sqlite-cli :as sqlite-cli]
            [logseq.outliner.validate :as outliner-validate]
            [nbb.core :as nbb]))
(defn -main [args]
  (let [{:keys [conn]} (sqlite-cli/open-sqlite-datascript! (first args))
        db @conn]
    (println "tag 4 =" (pr-str (:db/ident (d/entity db 4)))
             " title=" (pr-str (:block/title (d/entity db 4))))
    (println "\nAll pages whose name starts with $$$ :")
    (doseq [d (d/datoms db :avet :block/name)]
      (when (clojure.string/starts-with? (str (:v d)) "$$$")
        (println "  e=" (:e d) " name=" (pr-str (:v d))
                 " built-in=" (boolean (outliner-validate/built-in-entity? (d/entity db (:e d)))))))
    (println "\nFULL datom dump for panel 198:")
    (doseq [d (d/datoms db :eavt 198)]
      (println "   " (pr-str (:a d)) "=" (pr-str (:v d))))
    (println "\nFULL datom dump for page 188:")
    (doseq [d (d/datoms db :eavt 188)]
      (println "   " (pr-str (:a d)) "=" (pr-str (:v d))))))
(when (= nbb/*file* (nbb/invoked-file)) (-main *command-line-args*))
