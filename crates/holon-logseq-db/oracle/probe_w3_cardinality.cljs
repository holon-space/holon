(ns probe-w3-cardinality
  "Measure the two cardinality questions W3 carries, through LogSeq's OWN
   transactor and storage layer rather than by reading DataScript's source.

   Q1 — what does a REDUNDANT cardinality-many add do? Holon's `datoms_now`
        replays a tail assert by pushing it, which leaves two copies of an
        `(e, a, v)` the graph already holds.
   Q2 — can a graph carry a cardinality-many attribute its ROOT SCHEMA omits?
        Holon reads cardinality out of the root, so if the runtime schema can
        differ from the stored one, Holon's supersede path silently drops
        values.

   $ nbb-logseq -cp src script/probe_w3_cardinality.cljs GRAPH SCRATCH.sqlite"
  (:require ["fs" :as fs]
            [cljs-bean.core :as bean]
            [datascript.core :as d]
            [datascript.storage :as storage]
            [logseq.db.common.sqlite :as common-sqlite]
            [logseq.db.common.sqlite-cli :as sqlite-cli]
            [nbb.core :as nbb]))

(defn- tail-of [^js sqlite]
  (:content (first (bean/->clj (.all ^object (.prepare sqlite "select content from kvs where addr = 1"))))))

(defn- tags-of [db e]
  (sort (map :v (filter #(= :block/tags (:a %)) (d/datoms db :eavt e)))))

(defn -main [args]
  (let [[path scratch] args
        {:keys [sqlite conn]} (sqlite-cli/open-sqlite-datascript! path)
        db @conn
        ;; An entity that already carries a tag, so the add below is redundant.
        tagged (first (filter #(= :block/tags (:a %)) (d/datoms db :eavt)))
        e (:e tagged)
        v (:v tagged)]

    (println "=== Q1: a redundant cardinality-many add ===")
    (println "subject" e ":block/tags" (tags-of db e))
    (println "datoms before" (count (d/datoms @conn :eavt)))
    (println "tx of the held datom" (:tx (first (filter #(and (= :block/tags (:a %)) (= (:v %) v)) (d/datoms @conn :eavt e)))))
    (println "tail before" (tail-of sqlite))
    (d/transact! conn [[:db/add e :block/tags v]])
    (println "-- after [:db/add" e ":block/tags" v "] (redundant) --")
    (println "tags now" (tags-of @conn e))
    (println "datom count" (count (d/datoms @conn :eavt)))
    (println "tx of the held datom" (:tx (first (filter #(and (= :block/tags (:a %)) (= (:v %) v)) (d/datoms @conn :eavt e)))))
    (println "tail after" (tail-of sqlite))

    (println)
    (println "=== Q1b: a NON-redundant many add, as the control ===")
    (let [held (set (tags-of @conn e))
          other (first (remove held (map :e (filter #(= :block/uuid (:a %)) (d/datoms @conn :eavt)))))]
      (println "adding" other "which" e "does not already carry")
      (d/transact! conn [[:db/add e :block/tags other]])
      (println "tags now" (tags-of @conn e))
      (println "tail after" (tail-of sqlite)))

    (println)
    (println "=== Q1c: a redundant cardinality-ONE add ===")
    (let [title (:v (first (filter #(= :block/title (:a %)) (d/datoms @conn :eavt e))))
          tx-of (fn [] (map :tx (filter #(= :block/title (:a %)) (d/datoms @conn :eavt e))))]
      (println "title" (pr-str title) "held at tx" (tx-of))
      (d/transact! conn [[:db/add e :block/title title]])
      (println "after re-asserting the SAME title: tx" (tx-of))
      (println "tail after" (tail-of sqlite))
      (d/transact! conn [[:db/add e :block/title (str title "!")]])
      (println "after asserting a DIFFERENT title: tx" (tx-of))
      (println "tail after" (tail-of sqlite)))

    (println)
    (println "=== Q2: an attribute the root schema does not declare ===")
    (println "is :my.test/many in the restored schema?" (contains? (:schema @conn) :my.test/many))
    (d/transact! conn [[:db/add e :my.test/many "a"] [:db/add e :my.test/many "b"]])
    (println "values held for an UNDECLARED attribute after adding two:"
             (sort (map :v (filter #(= :my.test/many (:a %)) (d/datoms @conn :eavt e)))))

    (println)
    (println "=== Q2b: is the STORED root schema the runtime authority? ===")
    ;; A fresh graph whose schema deliberately differs from the one LogSeq
    ;; compiles in: if reopening it through LogSeq's own entry point (which
    ;; passes `db-schema/schema`) still reports `:my.test/many` as
    ;; cardinality-many, then `restore-conn` took the schema off the DISK and
    ;; the root is the authority Holon may read.
    (when (fs/existsSync scratch) (fs/unlinkSync scratch))
    (let [fresh (new sqlite-cli/sqlite scratch nil)
          _ (common-sqlite/create-kvs-table! fresh)
          store (sqlite-cli/new-sqlite-storage fresh)
          made (d/create-conn {:my.test/many {:db/cardinality :db.cardinality/many}}
                              {:storage store})]
      (d/transact! made [[:db/add 1 :my.test/many "a"] [:db/add 1 :my.test/many "b"]])
      (println "in the graph that DECLARED it many:" (sort (map :v (filter #(= :my.test/many (:a %)) (d/datoms @made :eavt 1)))))
      (storage/store @made)
      (.close fresh))
    (let [{reopened :conn} (sqlite-cli/open-sqlite-datascript! scratch)]
      (println "reopened through LogSeq's entry point (which passes its own compiled schema):")
      (println "  :my.test/many cardinality =" (get-in (:schema @reopened) [:my.test/many :db/cardinality]))
      (println "  values still held =" (sort (map :v (filter #(= :my.test/many (:a %)) (d/datoms @reopened :eavt 1)))))
      (println "  does the restored schema equal LogSeq's compiled one?"
               (= (:schema @reopened) (:schema @conn))))))

(when (= nbb/*file* (nbb/invoked-file))
  (-main *command-line-args*))
