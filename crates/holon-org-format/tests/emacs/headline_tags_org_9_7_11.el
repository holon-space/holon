;;; headline_tags_org_9_7_11.el --- how org reads a headline's title and tags  -*- lexical-binding: t -*-

;; Writes `headline_tags_org_9_7_11.txt'. Run from its directory:
;;   emacs -Q --batch -l headline_tags_org_9_7_11.el
;;
;; Each row is a headline line without its `* ', then the fields org-element
;; reads from `* LINE', separated by \x1f:
;;   LINE  RAW  TAGS  OFFSET  PLAIN  LINKED  UNLINKED
;; RAW is `:raw-value'. TAGS is `:tags' written as org writes a tag group
;; (`:a:b:'; empty for none; an empty tag stays empty: `:a::'). OFFSET is the
;; character offset of RAW in LINE. PLAIN is 1 when the title holds no org
;; object (it is one plain string). LINKED and UNLINKED are the headlines a
;; BlockToPage conversion can write, when org reads them back with the same
;; title and tag set: LINKED `* [[block:P][RAW]] :TAGS:' when org reads RAW as
;; the link's description and the link as the whole title, else empty;
;; UNLINKED `* RAW :TAGS:', else `!'. Both follow Holon's headline layout:
;; the priority cookie and a space before the title; one space, then the tags
;; sorted without empty tags or repeats; the line's own trailing blanks replace
;; the trailing spaces, and a space stays after the stars.

(require 'org)
(require 'org-element)
(require 'cl-lib)

(unless (equal (org-version) "9.7.11")
  (error "This fixture is measured with org 9.7.11, not %s" (org-version)))

(defconst fixture-alphabet '("a" ":" " " "\t" " " "_" "余" "é"))
(defconst fixture-small-alphabet '("a" ":" " " "_"))
(defconst fixture-hyphen-alphabet '("a" ":" " " "-"))

(defconst fixture-hand-cases
  '("ab:cd:" "x:a:" "x :a:" "x  :a:" "x :a:" "x\t:a:" "x \t :a: \t "
    "x :a b:" ":a b:" "x:a b:" "x ::" "x :::" "x ::::" "x:a::" "x :a::b:" "::a:"
    ":a:" "  :a:  " ":a:b:" ":_:" ":@:#:%:"
    "x :__:" "x :___:" "x :a__b:" "x :a___b:" "x :a__b:c___d:" "a__b:c___d:"
    "x :____:" "x :_a_:" "x :余:破:" "x :余: :破:" "余:破:"
    "x :Жß٣:" "x :a: :b:" "x :a: b" "x :a:b" "x a:" "x :a"
    "[#A] x :b:" "[#A] :b:" "[#A]:b:" "a ]] b :t:" "see [x] :t:" "see [[ here :t:"
    "*b* :t:" "*b*:t:" "/i/ x :t:" "x [[y]] :t:" "x [[y][z]]:t:" "[[y]]:t:"
    "Tagged title :work:urgent:" "  Single tagged :work:  " "a:b c:d:" "a :b:c d:e:"
    "bb:a-b:" "x :a-b:" "x :a-b:c:" "x :-:" "x :a:-:" "x - :a:" "x-:a:" "x :a_b-c:"))

(defun fixture-words (alphabet max-length)
  (let ((words '("")) (all '()))
    (dotimes (_ max-length)
      (setq words (cl-loop for w in words
                           append (cl-loop for c in alphabet collect (concat w c))))
      (setq all (append all words)))
    all))

(defun fixture-cases ()
  (cl-remove-duplicates
   (cl-remove-if
    (lambda (line) (string-match-p "\\`[ \t]*\\'" line))
    (append fixture-hand-cases
            (fixture-words fixture-alphabet 4)
            (fixture-words fixture-small-alphabet 6)
            (fixture-words fixture-hyphen-alphabet 5)))
   :test #'equal :from-end t))

(defun fixture-headline (text)
  (with-temp-buffer
    (org-mode)
    (insert text "\n")
    (goto-char (point-min))
    (let* ((tree (org-element-parse-buffer))
           (headline (org-element-map tree 'headline #'identity nil t)))
      (and headline
      (list (org-element-property :raw-value headline)
            (org-element-property :tags headline)
            (org-element-property :title headline)
            (org-element-map tree 'link
              (lambda (link)
                (list (org-element-property :raw-link link)
                      (and (org-element-contents-begin link)
                           (buffer-substring-no-properties
                            (org-element-contents-begin link)
                            (org-element-contents-end link)))))))))))

(defun fixture-tag-set (tags)
  (sort (delete-dups (delete "" (copy-sequence tags))) #'string<))

(defun fixture-tag-suffix (tags)
  (let ((set (fixture-tag-set tags)))
    (if set (concat " :" (mapconcat #'identity set ":") ":") "")))

(defun fixture-cookie (line)
  (if (string-match "\\`\\[#.\\]" line) (concat (match-string 0 line) " ") ""))

(defun fixture-trailing-blanks (line)
  (if (string-match "[ \t]+\\'" line) (match-string 0 line) ""))

(defun fixture-reads-back (written raw tags link)
  (pcase-let ((`(,raw2 ,tags2 ,_ ,links) (fixture-headline written)))
    (and (fixture-headline written)
         (equal (fixture-tag-set tags2) (fixture-tag-set tags))
         (if link
             (and (equal raw2 (concat "[[block:P][" raw "]]"))
                  (equal links (list (list "block:P" raw))))
           (equal raw2 raw)))))

(defun fixture-written (line title tags)
  (let ((written (concat "* " (fixture-cookie line) title (fixture-tag-suffix tags)))
        (blanks (fixture-trailing-blanks line)))
    (cond ((string= blanks "") written)
          ((and (string-match-p "\\`\\*+ *\\'" written)
                (not (string-prefix-p " " blanks)))
           (concat "* " blanks))
          (t (concat (string-trim-right written " +") blanks)))))

(defun fixture-linked (line raw tags)
  (let ((linked (fixture-written line (concat "[[block:P][" raw "]]") tags)))
    (if (and (not (string= raw "")) (fixture-reads-back linked raw tags t)) linked "")))

(defun fixture-unlinked (line raw tags)
  (let ((unlinked (fixture-written line raw tags)))
    (if (fixture-reads-back unlinked raw tags nil) unlinked "!")))

(defun fixture-offset (line raw tags)
  (let* ((group (if tags (concat ":" (mapconcat #'identity tags ":") ":") ""))
         (text (string-trim-right line "[ \t]+"))
         (text (string-trim-right (substring text 0 (- (length text) (length group)))
                                  "[ \t]+")))
    (unless (string-suffix-p raw text)
      (error "raw %S does not end %S (line %S)" raw text line))
    (- (length text) (length raw))))

(defun fixture-row (line)
  (pcase-let ((`(,raw ,tags ,title ,_) (or (fixture-headline (concat "* " line))
                                           (error "no headline in %S" line))))
    (mapconcat
     #'identity
     (list line
           raw
           (if tags (concat ":" (mapconcat #'identity tags ":") ":") "")
           (number-to-string (fixture-offset line raw tags))
           (if (or (null title) (and (= (length title) 1) (stringp (car title)))) "1" "0")
           (fixture-linked line raw tags)
           (fixture-unlinked line raw tags))
     "\x1f")))

(let ((coding-system-for-write 'utf-8-unix))
  (with-temp-file "headline_tags_org_9_7_11.txt"
    (dolist (line (fixture-cases))
      (insert (fixture-row line) "\n"))))
