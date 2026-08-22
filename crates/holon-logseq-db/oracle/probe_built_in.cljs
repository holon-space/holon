(ns probe-built-in
  "Ask LogSeq's OWN `outliner-validate/built-in-entity?` which entities of a
   graph it considers built-in, and emit the verdict as JSON.

   Holon's Rust predicate is pinned against THIS output, entity for entity, so
   the pin survives without the oracle installed and moves only when LogSeq's
   own answer moves. See docs/Testing/LogseqDbPush.md.

   Copied into the oracle checkout by the setup in docs/Testing/LogseqDbOracle.md.

   $ nbb-logseq -cp src:../outliner/src:../graph-parser/src:../common/src \\
       script/probe_built_in.cljs GRAPH [OUT.json]"
  (:require ["fs" :as fs]
            [datascript.core :as d]
            [logseq.db.common.sqlite-cli :as sqlite-cli]
            [logseq.outliner.validate :as outliner-validate]
            [nbb.core :as nbb]))

(defn -main [args]
  (let [[path out] args
        {:keys [conn]} (sqlite-cli/open-sqlite-datascript! path)
        db @conn
        eids (sort (distinct (map :e (d/datoms db :eavt))))
        ents (map #(vector % (d/entity db %)) eids)
        built-in (filterv (fn [[_ e]] (outliner-validate/built-in-entity? e)) ents)
        leg (fn [[_ e]]
              (cond (:logseq.property/built-in? e) "flag"
                    (:file/path e) "file-path"
                    :else "internal-ident"))
        payload {:entities (count eids)
                 :built_in (mapv first built-in)
                 :by_leg (frequencies (map leg built-in))
                 :non_built_in (mapv first (remove (fn [[_ e]]
                                                     (outliner-validate/built-in-entity? e))
                                                   ents))}]
    (println "entities:" (count eids)
             " built-in:" (count built-in)
             " by leg:" (pr-str (:by_leg payload)))
    (when out
      (fs/writeFileSync out (js/JSON.stringify (clj->js payload) nil 2))
      (println "wrote" out))))

(when (= nbb/*file* (nbb/invoked-file))
  (-main *command-line-args*))
