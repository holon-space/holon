;;; generate.el --- the keywords and properties org reads for a whole file -*- lexical-binding: t -*-
;; Regenerate (from this directory):
;;   emacs -Q --batch -l generate.el > org_keywords.txt
;; Section `file-keywords': every keyword `org-set-regexps-and-options'
;; collects when org-mode starts, every :keyword of the core
;; `org-export-options-alist', and the keywords org-macro (MACRO, and the
;; TITLE/AUTHOR/DATE/EMAIL templates) and ox (BIND, SETUPFILE) read file-wide.
;; Section `special-properties': `org-special-properties'.

(require 'org)
(require 'ox)

(defvar generate--collected nil)

(advice-add 'org-collect-keywords :before
            (lambda (keywords &rest _)
              (setq generate--collected (append keywords generate--collected))))

(with-temp-buffer
  (insert "#+TITLE: x\n* Row\n")
  (org-mode)
  (org-macro-initialize-templates)
  (let ((org-export-allow-bind-keywords t))
    (org-export-get-environment)))

(let ((keywords (append generate--collected
                        (delq nil (mapcar (lambda (option) (nth 1 option))
                                          org-export-options-alist))
                        '("SETUPFILE"))))
  (princ (format "# org %s, emacs %s\n" (org-version) emacs-version))
  (princ "[file-keywords]\n")
  (dolist (keyword (sort (delete-dups (mapcar #'upcase keywords)) #'string<))
    (princ (format "%s\n" keyword)))
  (princ "[special-properties]\n")
  (dolist (property (sort (copy-sequence org-special-properties) #'string<))
    (princ (format "%s\n" property))))
