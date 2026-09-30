# Org File ID Conventions

## Bare IDs in Org Files

Org files store IDs **without** scheme prefixes (`block:`, `sentinel:`).
The parser adds the correct `EntityUri` scheme when reading; the renderer strips it when writing.
Past that parse the scheme is mandatory: the operation boundary refuses an unschemed entity
reference (see [Operations.md](../Architecture/Operations.md#entity-references-are-parsed-once-at-the-dispatcher)).

### Link targets are the documented exception: they DO carry schemes

Link targets (`[[target]]` / `[[target][text]]`) are the one place the bare-ID
rule does not apply — a target is written and read with its full scheme, and the
renderer emits it verbatim. Targets are classified in three states:

| Target | Classified as | Example |
|---|---|---|
| Web/mail URL | external link, unchanged | `[[https://example.com][site]]` |
| Scheme-shaped, scheme **registered** as an entity | resolved entity URI | `[[block:abc]]`, `[[tag:rust]]`, `[[person:alice]]`, `[[cc-session:0f3a][the refactor]]` |
| Scheme-shaped, scheme **not registered** | unknown-scheme link — disclosed, bytes preserved, never a page | `[[Areas:Work]]`, `[[doc:x]]` (retired H7, 2026-07-02) |
| Any `/`-segment scheme-shaped | unknown-scheme link, same as above | `[[Areas/cc-session:abc]]` |
| Not scheme-shaped | page-creation intent, hashed to a deterministic `block:` UUID | `[[Projects/New thing]]`, `[[Ketosis: How to lose weight]]` |

"Scheme-shaped" is the RFC 3986 shape — `letter (letter|digit|+|-|.)* ':'` with
**no space after the colon**. The no-space rule is what keeps ordinary titles
(`Ketosis: How to lose weight`) on the page side without capitalization
heuristics, and Windows forbids `:` in filenames anyway, so a scheme-shaped page
file was never portable.

The shape is **reserved**: page creation rejects a scheme-shaped page name
(use `/` for hierarchy). That reservation is what makes installing or removing an
integration safe — its links move between resolved and unknown-scheme, never
across the page/entity boundary, so no page can have been silently minted under a
scheme that later becomes real.

The reservation is applied **per `/`-segment**, and the classifier applies the
writer's rule rather than a looser one of its own: `[[Areas/cc-session:abc]]` is
an unknown-scheme link, not a creation intent whose page `PageId::for_path` would
then refuse to mint. Over the scheme-shape rule the two accept exactly the same
set, so no link can carry a scheme-shaped intent that can never be fulfilled.

One known gap, not yet closed: the writer also refuses an **empty or
whitespace-only segment** (`[[a//b]]`, `[[Areas/]]`) as a malformed path, but
the classifier still calls those creation intents. `LinkTarget` has no honest
bucket for them — `UnknownScheme` means "scheme-shaped, scheme unclaimed" and
these carry no scheme — so closing it needs a new variant threaded through
`EntityRef`, its serde tag, the Loro codec, the org renderer and the link
styling. The failure is disclosed rather than silent: following such a link
calls the writer, which refuses loudly. Pinned by
`empty_segment_targets_still_diverge_from_the_writer`.

The registered set is the entity registry (`TypeRegistry`): built-ins plus every
entity a YAML sidecar declares. Registration is keyed by SQL table name
(underscored) while a scheme is hyphenated, so the lookup folds `-` to `_` —
without that fold every multi-word sidecar entity would silently read as an
unknown scheme. Pages are ordinary blocks tagged `Page`.

### File-level `:PROPERTIES:` drawer (document identity + preserved keys)

Standard Emacs org since 9.0 — and org-roam's default — puts the file's own
properties in a `:PROPERTIES:` drawer at the **very top** of the file, before any
`#+` keyword:

```org
:PROPERTIES:
:ID: 20260807T101010
:ROAM_REFS: https://example.com/paper
:END:
#+TITLE: Preserved
* First heading
```

Holon preserves it. The rules:

- **Position is part of the grammar.** Org (and orgize's `document_node`) only
  recognize a file-level drawer as the *first* element of the file. A
  `:PROPERTIES:` block that appears later is ordinary text and is treated as such.
- **`:ID:` is the document identity** — the same role this file gives `#+ID:`.
  Bare in the file, promoted to `block:<bare>` at the parse boundary. A file
  identified this way is rename-safe exactly like a `#+ID:` file.
- **`#+ID:` and a drawer `:ID:` together**: identical values are accepted (the
  file simply states its identity twice, and both carriers are written back).
  **Disagreeing values are rejected at the parse boundary** with an error naming
  the file and both ids. Silently preferring one would discard an authored
  identity — the failure this rule exists to prevent. There is no precedence
  order to remember.
- **Every non-`:ID:` key is preserved verbatim**, in the order the author wrote
  it, whether or not Holon models it (`:ROAM_REFS:`, `:CATEGORY:`, anything).
  They ride on the doc-root block under the `_file_properties` carrier (a
  JSON object; `serde_json::Map` preserves insertion order), so they survive the
  store round trip and are written back unchanged.
- **Write-back keeps the author's carrier.** A file whose drawer holds `:ID:` is
  re-emitted with the drawer first and **no** `#+ID:` line; a file with no drawer
  keeps using `#+ID:` as before. The drawer's `:ID:` value is always re-derived
  from the document's current identity, so the two can never drift apart.
- **Byte-identity holds for the canonical single-space form, NOT for padded
  files.** Emacs aligns drawer values (`org-property-format`, default `%-10s`),
  so a real org-roam file says `:ID:       20260807T101010` with padding. Holon
  re-emits every key as `:KEY: value` with ONE space and trims the value, in the
  same convergent sense as `:BLOCKED-BY:` → `:REQUIRES:` below: the first
  write-back rewrites that padding, and every write-back after it is a fixed
  point. No key and no value is lost — but do not expect a padded file to come
  back byte-for-byte, because it will not. If the padding matters to you, the
  fix is to preserve the authored whitespace per key, which this does not do.
- **An empty value keeps a trailing space** (`:KEY: `) in the file-level
  drawer. Holon's own reader keeps an empty value, so no literal is needed
  there (a headline drawer writes `""`, see below).
- **Indentation is allowed on input and CANONICALIZED on write.** Org's
  `drawer_begin_node` accepts leading horizontal space, so `  :PROPERTIES:` is
  still the file's drawer; Holon re-emits it at column 0. Same disclosure as the
  padding above: nothing is lost, the leading whitespace is not preserved.
- **A value-less `:KEY:` line is preserved** (re-emitted as `:KEY: `). Note that
  orgize's own property grammar rejects such a line and would void the entire
  drawer, which is why Holon reads this drawer itself rather than through the
  syntax tree — see `split_file_drawer`.
- **An empty `:ID:` does not identify the document.** It names nothing, so the
  document falls back to `#+ID:` or its name chain, and the empty value stays in
  the drawer exactly as authored — it is never filled in with the document's id.
- **A repeated key keeps its last value** (a `serde_json::Map` insert), and its
  first authored position.

| Role | File | Key function |
|------|------|--------------|
| Read the drawer + strip it from the body | `parser.rs` | `split_file_drawer()` — Holon owns this drawer; orgize never sees it |
| Cheap identity probe (same function) | `parser.rs` | `parse_file_drawer_from_content()` / `parse_doc_id_any_carrier()` |
| Fail loud if probe and parse ever disagree | `parser.rs` | the divergence check in `parse_org_file_with` |
| Reject disagreeing identity carriers | `parser.rs` | `resolve_document_identity()` |
| Carry it on the doc-root | `models.rs` | `OrgDocumentExt::file_drawer()` / `set_file_drawer()` (`org_props::FILE_PROPERTIES`) |
| Render it back | `models.rs` | `render_document_header()` emits the drawer before `#+ID:`/`#+TITLE:` |

### Heading blocks

```org
* My Heading
:PROPERTIES:
:ID: abc-123
:END:
```

- `:ID: abc-123` — bare string, no `block:` prefix
- Parser wraps with `EntityUri::from_raw("abc-123")` → `block:abc-123`
- Renderer writes `block.id.id()` (path part only) or `block.get_block_id()` (the stored "ID" property)

### Headline levels

A headline keeps its authored star count (`_stars`, recorded when it is not
one more than its parent's) while org reads it in the same place: more stars
than its parent, and at most as many as each earlier sibling. Otherwise, as
for a block Holon creates or moves, it is written one level below its parent.

The spaces and tabs at the end of a headline line are not part of the title
(org reads `* title ` as the title `title`). They are kept beside the block
(`_headline_end`) and written back at the end of the headline line.

A block's first line is its headline title and the rest is its section. Org
reads them as two elements, so no link or emphasis spans both: `* [[u][a`
followed by the body line `b]]` is text, not a link. A stored mark that spans
both cannot be written; the render drops it and reports a loss.

Converting a block to a page links the title text of its title line to the new
page (`parser.rs`, `linkable_title`): the trailing tag group stays outside the
link, so `* [[P][Tagged title]] :work:` keeps its tags. An org link
description ends at its first `]]` (`org-link-bracket-re`), and org writes a
`]]` or a final `]` in a description with a zero-width space that the text does
not hold. So a title that contains `]]` or ends in `]` converts with no page
link; nothing the file holds is lost. A `[[` in a title is description text to
org (`[[P][see [[ here]]`, measured with emacs -Q 30.2, org 9.7.11) and keeps
its link.

### Headline text: keyword, cookie, title, tags

The parser reads a headline as `stars [KEYWORD] [[#P]] title [:tags:]`
(`parser.rs`, `read_headline`). The tag group is the one org-element.el reads
(`org-element--headline-parse-title`, regexp `\(:[[:alnum:]_@#%:]+:\)[ \t]*$`
searched forward from the title):

- It is the leftmost run of tag characters and colons that ends the line,
  starting at its first colon, at least `:x:` long and ending in `:`. Only
  spaces and tabs may follow it: `* x :a:` followed by a form feed or a
  no-break space has no tags.
- No blank is needed before it: `* x:a:` is the title `x` with the tag `a`,
  and `* :a:b:` is an empty title with the tags `a`, `b`.
- An empty tag is no tag: `:a::b:` names `a` and `b`, and `* Foo :::` is the
  title `Foo` with no tags.
- The title is the text before the group with spaces and tabs (only those)
  trimmed: `* x\f` keeps the form feed.

`Tags::split_org_headline` (`crates/holon-api/src/types.rs`) and the orgize
fork's `headline_tags_node` implement this rule. Reference: the emacs -Q 30.2 /
org 9.7.11 fixture `crates/holon-org-format/tests/emacs/headline_tags_org_9_7_11.txt`
(9819 lines; regenerate with `emacs -Q --batch -l headline_tags_org_9_7_11.el`
in that directory). One difference is deliberate: **`-` is a tag character**
(org's class is `[[:alnum:]_@#%]`), for Logseq/Orgzly/Org-roam tags, so
`* Foo :a-b:` has the tag `a-b` here and the title `Foo :a-b:` in Emacs.
Rulings D21-D24 replace this with an org-boundary codec (`__`/`___`), under
which a legacy `:a-b:` refuses the headline; they are not implemented yet.

A title that org would read as ending in a tag group (`Foo :::`, `x:a:` with
no tags) does not read back as written, so the render records a loss.

Org has no escape for any of these parts. When the renderer writes a headline
that reads back with another keyword, cookie, title or tag set than the block
holds (`TODO buy milk` stored as text with no task state, `[#A] plan` with no
priority, a title `:t:` with no tags), the render records a loss, and
write-back raises `WritebackLossy` for that file
(`models.rs`, `check_headline_reads_back`). The keywords are the ones the file
declares in `#+TODO:`, else the defaults.

### Emphasis borders

Holon reads bold, italic, underline, strike-through, verbatim and code where
org-element.el does (`org-element--parse-generic-emphasis`):

- **Opening:** the marker is at the start of the line or the start of the
  object's contents (a link description, another emphasis), or follows one of
  `[[:space:]] - ( ' " {`. The next character is not `[[:space:]]`.
- **Closing:** the marker follows a character that is not `[[:space:]]`, and
  is followed by the end of the line or contents, or by one of
  `[[:space:]] - . , ; : ! ? ' " ) } \ [`.
- `[[:space:]]` is org-mode's whitespace class: space, tab, newline, form
  feed, carriage return, no-break space, U+2000-U+200B, U+202F, U+205F and
  U+3000. A vertical tab or U+1680 is not.
- The character before is the one in the text, also when it ends another
  object: `[[x]]/a/` has no italic, `*a*'/b/'` has one.

Inline source blocks and inline calls (`src_`, `call_`) need org's word start
`\<` instead: the character before is no word character of the same script
(`$`, `%`, `'`, Latin letters and digits, combining marks), so `a中src_x{y}`
holds a source block and `xsrc_x{y}` does not. Reference: the orgize fork's
emacs -Q 30.2 / org 9.7.11 fixtures `src/syntax/object_pre_org_9_7_11.txt`
(regenerate with `emacs -Q --batch -l object_pre_org_9_7_11.el` in that
directory) and `src/syntax/word_chars_org_9_7_11.rs` (generated by
`word_chars_org_9_7_11.el`), and `an_emphasis_border_is_read_as_org_reads_it`
in `src/syntax/emphasis.rs`.

### Block text

A block's text after its title line, and a page's text before its first
headline, is written below the headline. A line that org would read as a
headline (stars at the start of the line, then a blank or the end of the
line) or whose text starts with `#+` (after its indentation and any commas)
gets one more comma before that syntax, and the parser removes one. The comma
goes after the indentation, as org puts it: `  #+x: y` is written
`  ,#+x: y`. A line such as `*bold* text` is not a headline and is written as
it is. The block text `Shopping\n* milk\n#+TITLE: x` is written

```org
** Shopping
:PROPERTIES:
:ID: abc
:END:
,* milk
,#+TITLE: x
```

This is org's own escape inside source blocks
(`crates/holon-org-format/src/comma_escape.rs`). The delimiters of a block the
parser keeps as text (`#+begin_example`, `#+end_quote`, ...) are not escaped:
the parser keeps such a pair in the block's text, so it round-trips as
written, and vault pages use them, so escaping would turn their blocks into
literal text in Emacs. `#+begin_src`/`#+end_src` are escaped:
unescaped, the pair would become a source block child. A line `,* x` or
`,#+x` that a person writes in Emacs reads as `* x` / `#+x`.

Org itself removes an escape comma only inside example, export and source
blocks (measured in `emacs -Q`, `lane-logs/B10-emacs-contexts.log`), and there
before `*` or `#+` at any indentation: Holon writes and reads those lines the
same way. Inside any other block (quote, center, verse, comment, a special or
a dynamic block) and inside a drawer a comma is text: `:LOGBOOK:\n,x\n:END:`
reads as `,x`, and Holon writes no comma there, except before a headline. A
headline ends any block or drawer, so a line `* x` there is written `,* x`
and a line `,* x` there reads in Holon as `* x` (the D230.a rule below; org
reads `,* x`, measured in `emacs -Q` 30.2). A `#+KEY:` line there,
which org reads as a keyword, is written as it is, and the render records a
loss. Keyword lines org reads inside a quote, center or
special block or a list item are kept beside the block like section keyword
lines.

In paragraph text Holon deliberately differs from org (ruling D230.a, recorded
in `docs/Testing/bugfunnel/entries/2026-09-28-block-text-line-starting-with-a-star-becomes-a-new-block.md`):
org keeps a leading comma there, so Emacs shows Holon's `,* milk` with its
comma, and a file written in Emacs with a paragraph line `,* x` reads in
Holon as `* x`. Holon's own round trip is lossless, so no loss is recorded. A text that the renderer would write with
other bytes than the file holds (a raw `#+TODO:` line inside
`#+begin_example`, an unescaped `#+` line inside `#+BEGIN_SRC`) keeps its
authored bytes in the `_authored_text` carrier: the renderer writes them
while they still read as the block's text, and the escaped form once Holon
changes the text.

The page id is read only from a `#+ID:` keyword (key in any case: `#+id:`,
`#+Id:`) before the first headline, as org structures that text: a `#+ID:` line inside a block there (`#+begin_src`,
`#+begin_example`, ...) is not a keyword, and a line org reads as a headline
(`* x`, also inside such a block, as in Emacs) ends that text. Below a
headline, a `#+ID:` line is block text: the parser keeps it, so it is written
without a comma. The page's title and task keywords are read as org reads
them (`page_keywords.rs`, key in any case): the title from every `#+TITLE:`
line of the file, inside any block too (org-get-title scans the file), the
values joined with one space; the task keywords from every `#+TYP_TODO:`, then
`#+TODO:`, then `#+SEQ_TODO:` keyword element, above or below any headline and
inside quote, center or special blocks and list items, but not inside example,
export, source, verse or comment blocks. Each line adds its keywords; with no
`|`, its last keyword is a done state; a trailing `(...)` is a fast-access key
(`NEXT(n)` declares `NEXT`) and stays in the line. Each line stays where it
was authored. A title Holon changes goes into the first TITLE line Holon keeps
beside a block or in the header; the other such TITLE lines are removed, and
the render records a loss naming each. A TITLE line inside a block's text
stays, so the render records a loss when the title then reads otherwise. Changed
task keywords rewrite only the lines whose keywords changed, each with its
own kind and key spelling; a line left with no keyword is removed with a
loss; a new keyword goes into the first `#+TODO:` or `#+SEQ_TODO:` line, else
into a new `#+TODO:` header line. Every keyword line that a person writes below a headline in Emacs
(`#+STARTUP:`, `#+FOO:`, a `#+CAPTION:` above a table, a `#+TITLE:`) is not
block text: the parser keeps it beside the block (`_keyword_lines`,
`KeywordLine` in `models.rs`) with the body line it stands before, and the
renderer writes it back raw in its place. A `#+` line typed into a block's text
is text and is written comma-escaped, so both forms read back as they were.
The render's read-back check compares these lines too. A keyword line that
stands after a source block child is written before it, and the render records
a loss for the file. A carrier line that is no single keyword line is not
written, and the render records a loss.

The keyword lines before the first headline are written back as authored, in
their order and in their place among the text there, with their blank lines:
`#+FILETAGS:`, `#+STARTUP:`, `#+OPTIONS:`, a second `#+ID:`, any other. A line
that declares a value Holon keeps (the first `#+ID:`, `#+TITLE:`, `#+TODO:`,
`#+SEQ_TODO:` or `#+TYP_TODO:`) keeps its bytes while it still reads as that
value; when Holon changes the value, the line is regenerated in its place with
its authored key spelling. The renderer re-reads the written header and
records a loss when the page id, title or task keywords read back differently,
or when a header line is added, dropped, reordered or moved.

A raw `#+` line inside an example block below or above the first headline is
block text; it is written back raw while the text is unchanged, and
comma-escaped once Holon changes the text.

Inline marks see the text as the file holds it: a mark (a link, bold) may span
an escaped line and reads back with the same offsets and the same link target.

The renderer compares what it wrote with what the parser reads back, content
and inline marks included, and records a loss (`WritebackLossy`) for text the
file cannot hold: a title with leading or trailing blanks, a carriage return,
blank lines at the start or end of the text, a trailing line break, a body
line the parser takes as a child block (`[[file:x.png]]`), a mark org cannot
place (a link across a list item line). A render the parser would refuse (a title that
starts with `[#1]`) is not written at all, and the file keeps its old bytes.

`:PROPERTIES:`, `:END:` and `-----` lines in block text read back as text, and
the last line of a file keeps its trailing blanks.

Blank lines between a headline's drawer and its body, after a headline's
section (before the next headline, or at the end of the file), before a
page's first headline, at the start of the file, and before a source block
(after a body, another source block or the headline's head) are not block
text. The parser records each such line's bytes (a line of spaces or tabs
included) in the block's `_blank_lines` property (`BlankLines` in
`models.rs`; a source block's own are the lines before it), and the renderer
writes them back where they were. Not kept, with no loss raised:

- a list body followed directly by a headline gains the blank line that
  closes the list (not at the end of the file);
- a page created in Holon writes its text after its generated header and
  exactly one blank line;
- a file without a final line break gains one.

Text after a source block in the same section is written before the section's
source blocks; the render records a loss (`_text_after_source`).

A file whose every line ends with CRLF is written with CRLF. A file that mixes
CRLF and LF line breaks is written with LF, and the render records a loss for
the file.

### Source blocks

```org
#+BEGIN_SRC holon_sql :id abc-123::src::0
SELECT * FROM blocks
#+END_SRC
```

- `:id abc-123::src::0` — bare string in header args
- Parser wraps with `EntityUri::block(src_id)` → `block:abc-123::src::0`
- Renderer writes the bare id; an id no org id line holds is refused (see
  `DrawerId` above)
- Fallback ID (when no `:id` header arg): `{parent_id}::src::{index}` (e.g., `abc-123::src::0`)
- The `#+NAME:` line (any case, directly above), the `#+BEGIN_SRC` line and
  the `#+END_SRC` line are written as authored (`_source_lines`) while the
  block's name, language, header arguments and id read as they did. A block
  with no `:id` keeps that while it stays where the parser minted its id;
  elsewhere, or once edited, `:id` is written and the render records a loss.
- Every line between `#+BEGIN_SRC` and `#+END_SRC` is the block's text, blank
  lines at its start and end included, as org reads its `:value`.

### Rule blocks (`holon_rule`)

A `holon_rule` source block is a self-contained reactive rule (ADR 0024 §7.2):
its **body is YAML** carrying both the guard and the effect — no separate trigger
block. This supersedes the legacy query+action *pair* (a `holon_sql` trigger next
to a Rhai `block.create(...)` action). The default journal-auto-create rule:

```org
#+BEGIN_SRC holon_rule :id journals::action::0
name: daily_journal
when: 'not block_exists("Journals/{today}")'
emit:
  place: page(journals)
  name: "{today}"
#+END_SRC
```

- `when:` — a guard string parsed by the dual-evaluated `Pattern` AST
  (`block_exists`, `has_tag`, `and`/`or`/`not`; `{today}` interpolates the clock).
- `emit:` — a ratcheted create: `place` (the placement kind) + `name`
  (`{today}`-interpolated leaf content). `place: <root>` places an **inline child**
  of `block:<root>` (`journals` → `block:journals`); `place: page(<root>)` places a
  `Page`-tagged child that materializes into its own `<name-chain>.org` file. The
  journal rule ships `place: page(journals)` (LogSeq-parity daily-note ruling
  2026-07-19): each day is minted as a `Page`-tagged child of the journals shell
  that owns its own `Journals/{today}.org` file, so the day's bullets nest UNDER
  the date page (not as flat siblings of the shell) and the date is a first-class
  `[[{today}]]` link target. Companion de-inline (a rule-created child page would
  otherwise stay inlined in the `Journals.org` companion) is handled by the Fork B
  B1 writeback sweep.
- **A `Page`-tagged heading may nest only under another page.** A `:Page:` under a
  plain (non-page) heading is refused at ingest — its `<name-chain>` cannot be
  derived through a non-page ancestor, which contributes no path segment. This is a
  deliberate identity-model contract (ruled 2026-08-09), not a limitation; see
  [Architecture/Model.md § Page identity](../Architecture/Model.md) and the refusal
  site `DocumentManager::name_chain` (`crates/holon-filesystem/src/sync_ports.rs`).
- The block is **program-marked** (`is_program`) so it renders as a rule card, not
  as query content. A malformed body surfaces a loud `RuleStatus::ParseError` on
  the card. The parser is `holon_advice::holon_rule::parse_holon_rule`.

### Image blocks

An image child block renders as a single `[[file:…]]` link line inside its
parent heading's section:

```org
* Heading
:PROPERTIES:
:ID: abc-123
:END:
[[file:attachments/photo.png]]
```

- Canonical form: `[[file:<relative-path>]]` on its own line, where the path
  ends in a known image extension (`png`, `jpg`, `jpeg`, `gif`, `webp`, `svg`,
  `bmp`, `ico`, `tiff`, `tif`). The extension is what classifies the block as
  `ContentType::Image` at the **parse boundary** (`is_image_path`) — a
  `[[file:…]]` link to a non-image target (e.g. `.pdf`) stays inline text with a
  `Link` mark, it does NOT become an image block.
- `block.content` stores the bare path (no `file:` scheme, no brackets); the
  renderer re-adds `[[file:…]]` on write (`Block::to_org`).
- Fallback ID (images carry no `:id`): `{parent_id}::img::{index}` (e.g.
  `abc-123::img::0`), assigned by the parser in document order.
- Image blocks carry `marks = None` and `source_language = None`; the
  image-ness lives solely in `content_type = Image`, which every storage layer
  (org ⇄ SQL ⇄ Loro) must preserve. In Loro this is a first-class
  `BlockContent::Image { path }` variant so the create/read round-trip cannot
  silently collapse it to `Text`.

### Property values and keys that org cannot hold raw

One rule applies to every place org writes a property: a headline drawer, the
file-level drawer, and a source block's header arguments
(`crates/holon-org-format/src/drawer.rs`, `ValueCarrier`).

- **A value is written as it is when the parser reads it back unchanged.**
  Otherwise Holon writes it as a JSON string literal on one line, for example
  `:note: "line one\n* not a heading"`. This covers a line break, a value
  with space at the start or end, and a value that itself has the shape of
  such a literal. An empty value is written as nothing after `:KEY:`, which
  org reads as empty; an authored `""` is the two characters `""`, as org
  reads it. In header arguments, the literal also escapes each space, so it
  stays one token.
- **An authored value with the shape of such a literal reads differently in
  org.** Org reads a drawer value as its raw text. Holon decodes a value that
  is exactly what its encoder writes: an authored `" a"` reads as ` a`,
  `"a\nb"` as two lines, `"\t"` as a TAB, where org reads the quoted text
  (measured in `emacs -Q` 30.2). A quoted value the encoder does not write
  (`"The Book"`, `"x"`, `""`) reads as typed, as in org. The file bytes stay
  the same either way. Open:
  `docs/Testing/bugfunnel/entries/2026-09-30-quoted-empty-drawer-value-read-as-empty.md`.
- **A headline drawer value keeps the bytes it was authored with** (spacing
  around it, a whitespace-only value such as `:NOTE: `) while it still reads
  as the block's value: the parser records such bytes in `_drawer_raw`, and a
  value Holon changes is written by the rules above. A value-less `:NOTE:` line
  is valid org and reads as an empty value; the drawer and its `:ID:` are
  kept. The render re-reads each headline's `:ID:` and records a loss when it
  differs from the id Holon meant to write.
- **A headline drawer is read as org reads it** (`read_property_drawer`,
  measured in `lane-logs/B10-emacs-drawers.log`): every line between
  `:PROPERTIES:` and `:END:` (any case, indentation allowed) is `:KEY:` then a
  blank or the line's end; KEY has no whitespace and may hold colons
  (`:a:b: v` is key `a:b`). A drawer with a blank line, a text line, a key with
  a space or `:ID:aaa` is no property drawer for org: Holon reads no property
  from it, keeps the id its first `:ID:` line names (so the block keeps its
  identity), and every render records a loss saying that org finds no id
  there. The same holds for a PROPERTIES drawer after blank lines: it is no
  property drawer for org, and Holon keeps it with its blank lines. With two
  `:ID:` lines org reads the drawer, and org-entry-get (which org-id links
  resolve through) takes the last id; Holon takes the last too, keeps the
  drawer's bytes, and every render records a loss naming both ids. A
  PROPERTIES drawer after text is text to org (org-entry-get finds no id):
  Holon keeps the id its first `:ID:` line names, the drawer stays in the
  body as written, and every render records a loss; once the block holds
  another drawer value, a property drawer with that id is written above the
  text.
- **A drawer key that starts with `_` is a property like any other**, as org
  reads it (`_NOTE=keep me`), whatever its name: `:_drawer_raw: x` is a
  property named `_drawer_raw`, never a parser carrier. In the block's
  property bag, where the keys that start with `_` are Holon's own (the
  parser carriers, `_provenance`), such a key, and one that starts with `\`,
  is stored behind a `\` (`AuthoredKey`): `:_note: v` is the property
  `\_note`. The same holds for a source block's header arguments.
- **An unedited headline drawer keeps its bytes** (`_drawer_text`): spacing,
  key case, indentation, the place of `:ID:`, `:END:  `, a key the renderer
  cannot write. Once Holon changes the block's id or drawer values, the
  canonical drawer is written; a drawer org does not read then stays below it
  as text, and each dropped `:ID:` line (all but the last) is recorded as a
  loss.
- **Parser carriers are the parser's own** (`org_props::PARSER_CARRIERS`:
  `_drawer_raw`, `_keyword_lines`, `_header_lines`, `_blank_lines`,
  `_authored_text`, `_drawer_text`, ...). No file can write one: an authored
  `_` key is stored behind a `\`. The engine refuses an operation that writes
  one, unless the ingest or a peer writes it. A stored carrier the renderer
  cannot read is skipped, and the render records a loss.
- **The parser decodes a quoted value only when it is exactly the literal Holon
  would write.** A quoted value that a person types, such as
  `:title: "The Book"`, is not such a literal, so it stays as typed, quotes
  included. Every string goes into the file and comes back byte-equal.
- **A key must read back as itself, by the reader of its own carrier**
  (`ValueCarrier::key`, `DrawerKey`). Holon cannot escape a key, and each
  carrier has its own reader, so each has its own key rule:
  - **Headline drawer** (orgize): not empty; no whitespace, `:` or control
    character (orgize ends the key at the first space and needs a `:` after
    it; a bad key makes orgize reject the whole drawer); not `PROPERTIES` or
    `END` in any case; no trailing `+` (org's append syntax `:KEY+: value`,
    which orgize reads as `KEY`); not `ID` in any case (`id`, `Id`, `iD`: the
    parser takes the first of these as the block's identity and drops the
    others).
  - **File-level drawer** (Holon's own line reader, `parse_drawer_line`): not
    empty; no whitespace or `:`; not `PROPERTIES` or `END` in any case. The
    reader keeps a trailing `+` and control characters, so these keys are
    written back as authored. `ID` in any case is the identity line (below).
  - **Source-block header arguments** (`parse_header_args_from_str`): one
    whitespace-free token after the `:`, and not `id`, which is the source
    block's own id.

  The engine refuses a write under a key the carrier cannot hold: the
  headline rule for the `ID`/`properties`/`org_properties` routes (a block's
  properties may render as a headline drawer or as header arguments, and the
  headline rule is the stricter one), the file-level rule for
  `_file_properties`. The renderer leaves such a property out of the file (with
  a warning) and keeps the rest of the block.
- **Every org id line holds a bare block id, not a value** (`DrawerId`): a
  heading's `:ID:`, a source block's `:id` header argument, and a page's
  `#+ID:` or file-drawer `:ID:`. It is not encoded: the parser reads it
  verbatim and turns it into `block:<id>`. The rule is the contract itself:
  the id is non-empty, at most 255 bytes, forms the URI `block:<id>` with the
  same id (so no whitespace, control character, `#` or `?`), names no URI
  scheme of its own, and does not start with `:` (a source block's header
  arguments would read that as the next key). So `doc:x`, `block:x`,
  `sentinel:no_parent`, `a:b` and `:split-1` are refused, while `12:34` (a
  scheme cannot start with a digit), `x::src::0`, `Notes/Sub.md::b::0` (the
  Markdown adapters' ids) and `a%20b` are bare. The engine refuses any other
  id on every route that can carry one: the `ID` property, the `properties`
  bag, the `org_properties` carrier, the `_file_properties` carrier (where an
  empty `:ID:` is allowed, since it is authored text and not an identity), and
  the `id` of a `create`, which must be `block:<id>`. The parser refuses a
  file whose heading `:ID:`, source block `:id` or page id is not a bare block
  id, naming the id. If such an id still reaches the renderer (a block or page
  id with another scheme, or a carrier `ID`), the render fails with an error
  naming it and the write-back leaves that file as it was, disclosed with an
  ERROR, while the other files of the pass are written; no id is ever written
  in its place. A page with a `file:` id keeps its path identity and gets no
  id line.
- **Keys the parser lifts into typed fields are not plain properties.** The
  headline drawer keys `ID`, `REQUIRES`, `BLOCKED-BY`, the other edge fields
  in column or kebab spelling (`tags`, `advice_suppressed`,
  `contributes-to`, ...), `priority`, `COLLAPSED` and `WIDGET_ONLY` (any case)
  become typed block fields, and the parser refuses `task_state` and
  `task_state_category` as drawer keys (`drawer.rs`, `TypedDrawerKey`). The
  engine refuses a property write under any of these spellings, through a
  properties bag or as a flat property field, and names the key. A flat write
  of an edge column (`requires`, `contributes_to`) and of `priority` (which
  the store keeps in the properties bag) is that field's own write. The
  file-level drawer is read verbatim, so these keys are plain there.

### Why bare IDs?

1. **Human readability** — org files are edited in Emacs/vim, scheme prefixes are noise
2. **URI parsing ambiguity** — bare IDs like `j-09-::src::0` can be mis-parsed as scheme `j-09-` with path `::src::0` by RFC 3986 parsers. By convention, org files always store bare IDs and the parser always wraps them.
3. **Single source of truth** — the `EntityUri` type enforces the scheme internally; the org file just stores the identity

## Edge-field drawers: dependency edge (`:REQUIRES:` / `:BLOCKED-BY:`)

A block's dependency edge — "this block is blocked by / depends on these
blocks" — is a single edge field (`Block.requires`, projected to the
`block_requires` junction; see `crates/holon-turso/sql/schema/block_requires.sql`,
whose own comment names both spellings for this one edge). It has **two accepted
org-drawer spellings on read** and **one canonical spelling on write**:

| Spelling | On read (parse) | On write (render) |
|----------|-----------------|-------------------|
| `:REQUIRES: a b`   | lifted into `Block.requires` | **canonical** — always emitted |
| `:BLOCKED-BY: a b` | lifted into `Block.requires` (accepted alias) | never emitted (converges to `:REQUIRES:`) |

- Values are bare IDs (whitespace- or comma-separated), promoted to `block:`
  URIs at the parse boundary and stripped back to bare on render, exactly like
  IDs above.
- **Canonical form is `:REQUIRES:`** (owner ruling 2026-07-16). A `:BLOCKED-BY:`
  drawer therefore converges to `:REQUIRES:` on the first write-back — a
  *convergent canonical form* (the org analogue of the foreign-vault O4 ruling,
  `docs/Proposals/ForeignVaultCompat-2026-07-12.md` §6a): the edge is preserved
  losslessly; only the interchangeable keyword normalizes, and it reaches a
  fixed point in one pass.
- Rendered targets are **sorted** (the edge is a set of blockers; order is not
  semantic), which also makes the round-trip deterministic through the
  `json_group_array` junction hydration (no `ORDER BY`).
- There is **no distinct `BlockedBy` edge field**: `EdgeField` enumerates only
  `Tags`, `Requires`, `AdviceSuppressed` (`crates/holon-api/src/edge_field.rs`).
  `:BLOCKED-BY:` is an accepted input surface spelling of the `Requires` edge,
  not a second junction.

| Role | File | Key function/line |
|------|------|-------------------|
| Parse `:REQUIRES:`/`:BLOCKED-BY:` drawer | `parser.rs` | headline loop unions both keys into `block.requires` |
| Parse `:REQUIRES`/`:BLOCKED-BY` src header-arg | `parser.rs` | source-block header-arg loop unions both keys |
| Render canonical `:REQUIRES:` | `models.rs` | `drawer_properties()` inserts sorted `REQUIRES` |

### Code locations

| Role | File | Key function/line |
|------|------|-------------------|
| Parse heading ID | `parser.rs` | `EntityUri::from_raw(&id)` at block creation |
| Parse source ID | `parser.rs` | `EntityUri::block(&src_id)` at source block creation |
| Render heading ID | `models.rs` | `format_properties_drawer()` writes `ID` property (already bare) |
| Render source ID | `models.rs` | `source_block_to_org()` writes `block.id.id()` |
| Sync controller | `file_sync_controller.rs` | `build_block_params()` uses `get_block_id()` with `block.id.id()` fallback |
| Test serializer | `org_utils.rs` | `serialize_block_recursive()` writes `block.id.id()` |

## Task keywords: the `?` question

`?` is a default active keyword (`DEFAULT_ACTIVE_KEYWORDS`,
`crates/holon-org-format/src/models.rs`): `* ? pick a storage engine` is an open
question, task state `?`. A document may declare it in `#+TODO:`, on either side
of the `|`; its category then comes from that declaration.

A `?` asks something only when text follows it on the same line. A bare `* ?`
is the title text `?`, not a task: the parser, the store's convergence and the
editor apply the same rule (`asks_nothing` in `task_keyword.rs`). A task state
is written only as the headline keyword; a `:task_state:` or
`:task_state_category:` drawer key refuses the file.

Own writes do not produce task state `?` over empty content:

| Write | Result |
|-------|--------|
| `set_field task_state "?"`, `create`/`update` with `task_state: "?"`, on empty content | refused with an error that names the block |
| `cycle_task_state` on a block with no text | the ring skips `?` |
| a content write that empties a `?` block | the same gesture clears the task state |
| org ingest of a headline that no longer carries a keyword (`* ?`, `* `, `* buy milk`) | the stored task state is cleared |

A merge of concurrent peer edits (one clears the text, one sets `?`) can still
produce the state. The editor then shows the stored content and discloses the
refusal (`QuestionWithoutText`), and the keyword is not editable there.

Views that could treat an open `?` block as work:

| Site | Verdict |
|------|---------|
| Vault `Now.org` now-query and `now_for_agent` MCP tool | selects TODO/DOING only; `?` excluded |
| Vault `Now.org` REQUIRES rollup | dependents of an open `?` stay blocked until it is answered |
| `rank_tasks` (Petri `TaskInfo::from_block`) | open `?` is not ranked and blocks its dependents; a `?` declared done frees them |
| Petri `? ` content prefix | separate question-arc syntax, unaffected |
| `dense_query` `task_state_category != 'done'` example | lists open `?` blocks as open items |
| Kanban lanes (`collection_profile.yaml`) | `?` gets its own lane |
| `state_icon` / `state_display` (`render_eval.rs`) | open-task glyph |
| Native cycle ring (`task_keyword_cycle.rs`) | `?` is not in it; a click on `?` goes to TODO |
| `TaskEntity::completed` (`holon-core/src/traits.rs`) | open `?` is not completed; ignores the category sidecar |
