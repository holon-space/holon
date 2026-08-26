//! Certifies `crates/holon-org-format/profile.yaml` against the REAL org
//! round trip — Increment 2b.1, axes 3 (`property_keys`) and 4
//! (`property_values`).
//!
//! The `CertifiableFormat` impl lives HERE, in an integration-test target,
//! never in `src/`. `holon-org-format` must not gain a non-test dependency on
//! `holon-capability`, or the format crate would start reading its own profile
//! at runtime and the profile would stop being an independent statement ABOUT
//! it. Pinned by `crates/holon-architecture-tests/tests/architecture_rules.rs`.
//!
//! ## Org has TWO property carriers and they fail DIFFERENTLY
//!
//! That is why every finding names its leg:
//!
//! * `org_properties_json` — the drawer is rendered straight from the
//!   `org_properties` JSON string. A non-string is STRINGIFIED
//!   (`models.rs:186-189` and `:199-202`, `_ => value.to_string()`), so an
//!   integer reaches disk as the text `42` and returns as `Value::String`.
//! * `flat_properties` — no `org_properties` is set, so the renderer rebuilds
//!   the drawer from `drawer_properties()`, whose flat leg gates on
//!   `Value::as_string()` (`models.rs:910`). `as_string` is `None` for every
//!   non-`String` variant (`crates/holon-pattern/src/value.rs:85-90`), so the
//!   value is DROPPED.
//!
//! Both legs erase `_`-prefixed keys (`models.rs:886`, `:894`, `:909`).

use std::path::Path;

use holon_api::EntityUri;
use holon_api::Tags;
use holon_api::Value;
use holon_capability::BlockConstruct;
use holon_capability::CapabilityProfile;
use holon_capability::Carrier;
use holon_capability::CertifiableFormat;
use holon_capability::ConstraintId;
use holon_capability::ConstructOutcome;
use holon_capability::DisagreementOutcome;
use holon_capability::Extension;
use holon_capability::IdCarrier;
use holon_capability::InlineConstruct;
use holon_capability::Leg;
use holon_capability::MultiValueReadback;
use holon_capability::Readback;
use holon_capability::ReferenceReadback;
use holon_capability::WriteAttempt;
use holon_capability::WriteLeg;
use holon_capability::certify;
use holon_org_format::OrgBlockExt;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;

const ROOT: &str = "/certify";
const FILE: &str = "/certify/profile.org";
const BLOCK_ID: &str = "certify-block";
const PROBE_HEADLINE: &str = "Probe headline";

/// A one-headline document carrying nothing but its identity. Every probe
/// starts from this, ingested, so the property under test is the ONLY thing
/// that varies.
const BASE_FIXTURE: &str = "#+TITLE: Certify\n\n* Probe headline\n:PROPERTIES:\n:ID: \
                            certify-block\n:END:\n";

const JSON_LEG: Carrier = Carrier {
    leg: Leg("org_properties_json"),
    description: "drawer rendered from the org_properties JSON string",
};
const FLAT_LEG: Carrier = Carrier {
    leg: Leg("flat_properties"),
    description: "drawer rebuilt from the flat properties bag",
};

struct OrgFormat {
    profile: CapabilityProfile,
}

impl OrgFormat {
    /// The crate's own profile, or the one `HOLON_CAPABILITY_PROFILE` names.
    ///
    /// Read at RUNTIME rather than `include_str!`-ed so
    /// `scripts/capability-flip-sweep.sh` can certify a mutated COPY under
    /// `target/`: a sweep that edited `profile.yaml` in place would be writing
    /// into the source tree on every flip.
    fn load() -> Self {
        let path = std::env::var_os("HOLON_CAPABILITY_PROFILE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("profile.yaml"));
        Self {
            profile: CapabilityProfile::from_path(&path)
                .unwrap_or_else(|e| panic!("the org profile must load: {e:#}")),
        }
    }

    /// Round-trip a WHOLE authored file and report whether `markers` returned.
    ///
    /// Used by the structural probes, where the fixture is the file itself
    /// rather than a body under a fixed headline. A parse `Err` is `Refused` —
    /// the law's other legal branch — not a harness failure.
    fn survives_in_place(&self, src: &str, markers: &[&str]) -> anyhow::Result<ConstructOutcome> {
        let path = Path::new(FILE);
        let parsed = match parse_org_file(path, src, &EntityUri::no_parent(), Path::new(ROOT)) {
            Ok(p) => p,
            Err(e) => {
                return Ok(ConstructOutcome::Refused {
                    reason: e.to_string(),
                });
            }
        };
        let rendered = OrgRenderer::render_document(
            &parsed.document,
            &parsed.blocks,
            path,
            &parsed.document.id,
        )
        .to_lowercase();
        let present = markers
            .iter()
            .filter(|m| rendered.contains(&m.to_lowercase()))
            .count();
        Ok(if present == markers.len() {
            ConstructOutcome::Survived
        } else if present > 0 {
            ConstructOutcome::Changed { got: rendered }
        } else {
            ConstructOutcome::Lost
        })
    }

    /// Set a block's STRUCTURED tag set, render, parse back, and report what
    /// the RE-PARSED block carries.
    ///
    /// This reads `block.tags` on the far side rather than scanning the
    /// rendered bytes, so it measures the set the model holds — the leg from
    /// `tags.to_org()` (models.rs:1353-1358) to the headline tag lift
    /// (parser.rs:823). MEASURED which of the two `headline.tags()` calls that
    /// is: neutering :823 empties the staged set and trips the carried-assert
    /// below, while neutering :578 changes the run not at all — so :823 is the
    /// path a certified block travels and :578 serves something else.
    ///
    /// WHAT THIS DOES NOT GIVE ORG, stated so nobody reads more into it: for
    /// org the structured set and the `:sometag:` syntax are ONE observable —
    /// both readbacks come from the same headline tag lift, and no break
    /// separates them. Org's structure IS its syntax. The value of the tags
    /// clauses here is cross-format COMPARABILITY (org `carried` against
    /// logseq-db `refused` against native `carried`), not an org-local fact
    /// that `content.inline_constructs: tag` was missing.
    fn tags_round_trip(&self, authored: &[&str], wanted: &[&str]) -> anyhow::Result<Tags> {
        let path = Path::new(FILE);
        let base = parse_org_file(path, BASE_FIXTURE, &EntityUri::no_parent(), Path::new(ROOT))
            .map_err(|e| anyhow::anyhow!("the base fixture must parse: {e}"))?;
        let doc = base.document.clone();
        let mut block = base
            .blocks
            .iter()
            .find(|b| b.org_title() == PROBE_HEADLINE)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("the base fixture must carry the probe block"))?;

        // Author a starting set, so a detach has a subject that really was
        // there — "gone" must not be true before the write.
        block.tags = Tags::from_tag_iter(authored.iter().map(|t| (*t).to_string()));
        let authored_src = OrgRenderer::render_document(&doc, &[block.clone()], path, &doc.id);
        let staged = parse_org_file(
            path,
            &authored_src,
            &EntityUri::no_parent(),
            Path::new(ROOT),
        )
        .map_err(|e| {
            anyhow::anyhow!("the authored fixture must parse back: {e}\n{authored_src}")
        })?;
        let mut block = staged
            .blocks
            .iter()
            .find(|b| b.org_title() == PROBE_HEADLINE)
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!("the probe block vanished while staging:\n{authored_src}")
            })?;

        // The staged set must REALLY have carried before the transition is
        // measured. Without this the staging is decorative: `wanted` is
        // written over whatever came back, so a detach would assert a name
        // absent that was never in the measured input — vacuous however the
        // staging behaved.
        for tag in authored {
            anyhow::ensure!(
                block.tags.contains(tag),
                "the staged attach must really carry `{tag}` before the transition is measured; \
                 got {:?} from\n{authored_src}",
                block.tags
            );
        }

        block.tags = Tags::from_tag_iter(wanted.iter().map(|t| (*t).to_string()));
        let rendered =
            OrgRenderer::render_document(&staged.document, &[block], path, &staged.document.id);
        let parsed = parse_org_file(path, &rendered, &EntityUri::no_parent(), Path::new(ROOT))
            .map_err(|e| {
                anyhow::anyhow!("the rendered fixture must parse back: {e}\n{rendered}")
            })?;
        Ok(parsed
            .blocks
            .iter()
            .find(|b| b.org_title() == PROBE_HEADLINE)
            .ok_or_else(|| {
                anyhow::anyhow!("the probe block vanished from the round trip:\n{rendered}")
            })?
            .tags
            .clone())
    }

    /// Author `body` under a headline, write it back, and report whether the
    /// construct returned.
    ///
    /// `markers` is what must still be present, case-insensitively, for the
    /// construct to count as survived. Deliberately NOT a byte-equality check
    /// on the whole body: the renderer legitimately canonicalises
    /// (`#+begin_src` → `#+BEGIN_SRC`) and adds its own source id, and calling
    /// that a loss would report the format's disclosed normalisation as a
    /// defect. Each marker names the part of the construct that MUST survive.
    fn body_survives(&self, body: &str, markers: &[&str]) -> anyhow::Result<ConstructOutcome> {
        let path = Path::new(FILE);
        let src = format!(
            "#+TITLE: Certify\n\n* {PROBE_HEADLINE}\n:PROPERTIES:\n:ID: {BLOCK_ID}\n:END:\n{body}\n"
        );
        let parsed = match parse_org_file(path, &src, &EntityUri::no_parent(), Path::new(ROOT)) {
            Ok(p) => p,
            // A construct the parser REFUSES is the law's other legal branch,
            // not a harness failure.
            Err(e) => {
                return Ok(ConstructOutcome::Refused {
                    reason: e.to_string(),
                });
            }
        };
        let rendered = OrgRenderer::render_document(
            &parsed.document,
            &parsed.blocks,
            path,
            &parsed.document.id,
        )
        .to_lowercase();

        let present = markers
            .iter()
            .filter(|m| rendered.contains(&m.to_lowercase()))
            .count();
        Ok(if present == markers.len() {
            ConstructOutcome::Survived
        } else if present > 0 {
            // Some of the construct came back and some did not — the
            // accept-then-alter outcome, distinct from losing it outright.
            ConstructOutcome::Changed { got: rendered }
        } else {
            ConstructOutcome::Lost
        })
    }
}

/// A `Value` as the `org_properties` JSON carrier would hold it — the shape
/// the renderer's `serde_json::Value` match arms actually see.
fn as_json(value: &Value) -> serde_json::Value {
    match value {
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Integer(i) => serde_json::Value::from(*i),
        Value::Float(f) => serde_json::Value::from(*f),
        Value::Boolean(b) => serde_json::Value::Bool(*b),
        Value::DateTime(s) => serde_json::Value::String(s.clone()),
        Value::Json(s) => {
            serde_json::from_str(s).unwrap_or_else(|_| serde_json::Value::String(s.clone()))
        }
        Value::Array(items) => serde_json::Value::Array(items.iter().map(as_json).collect()),
        Value::Object(map) => {
            serde_json::Value::Object(map.iter().map(|(k, v)| (k.clone(), as_json(v))).collect())
        }
        Value::Null => serde_json::Value::Null,
        Value::Removed(_) => panic!("as_json: the removal sentinel is not a certifiable value"),
    }
}

impl CertifiableFormat for OrgFormat {
    fn profile(&self) -> &CapabilityProfile {
        &self.profile
    }

    fn carriers(&self) -> &'static [Carrier] {
        &[JSON_LEG, FLAT_LEG]
    }

    fn round_trip_property(
        &self,
        carrier: Carrier,
        key: &str,
        value: &Value,
    ) -> anyhow::Result<Readback> {
        let path = Path::new(FILE);
        // The base comes from a real INGEST, not a hand-built Block: the
        // round trip under test is the production loop (parse → mutate →
        // render → parse), so the fixture must enter it the way a vault file
        // does.
        let base = parse_org_file(path, BASE_FIXTURE, &EntityUri::no_parent(), Path::new(ROOT))
            .map_err(|e| anyhow::anyhow!("the base fixture must parse: {e}"))?;
        let doc = base.document.clone();
        let mut block = base
            .blocks
            .iter()
            .find(|b| b.org_title() == PROBE_HEADLINE)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("the base fixture must carry the probe block"))?;

        match carrier.leg {
            Leg("org_properties_json") => {
                let mut props = serde_json::Map::new();
                props.insert("ID".to_string(), serde_json::Value::String(BLOCK_ID.into()));
                props.insert(key.to_string(), as_json(value));
                block.set_org_properties(Some(serde_json::to_string(&props)?));
            }
            Leg("flat_properties") => block.set_property(key, value.clone()),
            other => anyhow::bail!("org has no carrier named {other}"),
        }

        let rendered = OrgRenderer::render_document(&doc, &[block], path, &doc.id);
        let parsed = parse_org_file(path, &rendered, &EntityUri::no_parent(), Path::new(ROOT))
            .map_err(|e| {
                anyhow::anyhow!("the rendered fixture must parse back: {e}\n{rendered}")
            })?;

        // Located by HEADLINE, not by id: a probe key is allowed to be one the
        // format owns, and writing `ID` changes the block's identity. Looking
        // it up by id would turn that legitimate case into a harness failure.
        let back = parsed
            .blocks
            .iter()
            .find(|b| b.org_title() == PROBE_HEADLINE)
            .ok_or_else(|| {
                anyhow::anyhow!("the probe block vanished from the round trip:\n{rendered}")
            })?;

        Ok(match back.get_property(key) {
            Some(v) => Readback::Present(v),
            None => Readback::Absent,
        })
    }

    /// Drives `ordering.property_order` through the REAL write-back path.
    ///
    /// The fixture is authored in a deliberately UNSORTED order, so a format
    /// that alphabetizes cannot pass by accident. What is under test is the
    /// `_drawer_order` carrier (`models.rs:40-43`): it records the author's key
    /// order in the STORED properties bag and the renderer replays it — which
    /// is why the claim survives despite `_` being a reserved prefix in this
    /// same profile.
    fn round_trip_property_order(&self, authored: &[&str]) -> anyhow::Result<Option<Vec<String>>> {
        let path = Path::new(FILE);
        let drawer: String = authored
            .iter()
            .map(|k| format!(":{k}: v-{k}\n"))
            .collect::<Vec<_>>()
            .join("");
        let src = format!(
            "#+TITLE: Certify\n\n* {PROBE_HEADLINE}\n:PROPERTIES:\n:ID: {BLOCK_ID}\n{drawer}:END:\n"
        );

        let parsed = parse_org_file(path, &src, &EntityUri::no_parent(), Path::new(ROOT))
            .map_err(|e| anyhow::anyhow!("the ordering fixture must parse: {e}"))?;
        let block = parsed
            .blocks
            .iter()
            .find(|b| b.org_title() == PROBE_HEADLINE)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("the ordering fixture must carry the probe block"))?;
        let rendered =
            OrgRenderer::render_document(&parsed.document, &[block], path, &parsed.document.id);

        // Read the ORDER off the rendered drawer, not off a map: the claim is
        // about what reaches disk, and a HashMap read-back would hide a
        // renderer that reorders.
        let order: Vec<String> = rendered
            .lines()
            .filter_map(|l| {
                let rest = l.trim().strip_prefix(':')?;
                let (key, _) = rest.split_once(':')?;
                let key = key.trim();
                (!key.is_empty()
                    && !key.eq_ignore_ascii_case("PROPERTIES")
                    && !key.eq_ignore_ascii_case("END")
                    && !key.eq_ignore_ascii_case("ID")
                    && authored.contains(&key))
                .then(|| key.to_string())
            })
            .collect();
        Ok(Some(order))
    }

    /// Drives `content.block_constructs` — EVERY construct in the closed
    /// vocabulary, declared or not.
    ///
    /// Driving the undeclared ones is how the draft's UNKNOWNs get resolved:
    /// `table` and `logbook` were declared absent because recon found no parser
    /// support, and 2b.5 may not refuse content on a clause nobody drove. If
    /// either round-trips, the certifier raises a prompt and the profile was
    /// too narrow; if it does not, "absent" is now MEASURED rather than
    /// assumed.
    fn round_trip_block_construct(
        &self,
        construct: BlockConstruct,
    ) -> anyhow::Result<Option<ConstructOutcome>> {
        // One authored specimen per construct, as a whole file. `None` for the
        // constructs whose specimen would be the fixture itself.
        let (body, markers): (&str, &[&str]) = match construct {
            BlockConstruct::Table => (
                "| a | b |\n|---+---|\n| 1 | 2 |",
                &["| a | b |", "| 1 | 2 |"],
            ),
            BlockConstruct::Logbook => (
                ":LOGBOOK:\nCLOCK: [2026-08-22 Fri 10:00]\n:END:",
                &[":logbook:", "clock: [2026-08-22 fri 10:00]"],
            ),
            BlockConstruct::Quote => (
                "#+begin_quote\nquoted\n#+end_quote",
                &["begin_quote", "quoted", "end_quote"],
            ),
            BlockConstruct::List => ("- one\n- two", &["- one", "- two"]),
            // The renderer canonicalises the keyword case and adds its own
            // `:id` param — disclosed normalisation, so the markers ask for the
            // keyword and the PAYLOAD, not the authored header verbatim.
            BlockConstruct::SourceBlock => (
                "#+begin_src prql\nfrom x\n#+end_src",
                &["begin_src", "from x", "end_src"],
            ),
            BlockConstruct::Paragraph => ("just a paragraph", &["just a paragraph"]),
            // The five the per-member law exposed as declared-but-undriven.
            // Each is HEADLINE-level, so the specimen is a whole file rather
            // than a body — see `headline_survives`.
            BlockConstruct::Heading
            | BlockConstruct::Image
            | BlockConstruct::PlanningTimestamp
            | BlockConstruct::TodoKeyword
            | BlockConstruct::Priority => {
                let (src, markers): (&str, &[&str]) = match construct {
                    BlockConstruct::Heading => (
                        "#+TITLE: Certify\n\n* A heading\n:PROPERTIES:\n:ID: h-1\n:END:\n",
                        &["* a heading"],
                    ),
                    BlockConstruct::Image => (
                        "#+TITLE: Certify\n\n* Head\n:PROPERTIES:\n:ID: \
                         h-1\n:END:\n[[file:pic.png]]\n",
                        &["[[file:pic.png]]"],
                    ),
                    BlockConstruct::PlanningTimestamp => (
                        "#+TITLE: Certify\n\n* Head\nSCHEDULED: <2026-08-22 \
                         Fri>\n:PROPERTIES:\n:ID: h-1\n:END:\n",
                        &["scheduled: <2026-08-22 fri>"],
                    ),
                    BlockConstruct::TodoKeyword => (
                        "#+TITLE: Certify\n\n* TODO Head\n:PROPERTIES:\n:ID: h-1\n:END:\n",
                        &["* todo head"],
                    ),
                    BlockConstruct::Priority => (
                        "#+TITLE: Certify\n\n* [#A] Head\n:PROPERTIES:\n:ID: h-1\n:END:\n",
                        &["[#a]"],
                    ),
                    _ => unreachable!("the arm above lists exactly these five"),
                };
                return Ok(Some(self.survives_in_place(src, markers)?));
            }
            // Heading / Image / PlanningTimestamp / TodoKeyword / Priority are
            // headline-level, not body content: they need their own fixture
            // shapes, which the headline pass covers. Reporting `None` is
            // honest — the coverage law then requires the marker rather than
            // letting a silent skip look like a pass.
            _ => return Ok(None),
        };
        Ok(Some(self.body_survives(body, markers)?))
    }

    /// Drives `content.inline_constructs` the same way, through the headline.
    fn round_trip_inline_construct(
        &self,
        construct: InlineConstruct,
    ) -> anyhow::Result<Option<ConstructOutcome>> {
        let body = match construct {
            InlineConstruct::Bold => "*bold*",
            InlineConstruct::Italic => "/italic/",
            InlineConstruct::Underline => "_underline_",
            InlineConstruct::Strikethrough => "+struck+",
            InlineConstruct::Verbatim => "=verbatim=",
            InlineConstruct::Code => "~code~",
            InlineConstruct::Subscript => "a_{sub}",
            InlineConstruct::Superscript => "a^{sup}",
            InlineConstruct::LinkExternal => "[[https://example.com][site]]",
            InlineConstruct::LinkByName => "[[Some Page]]",
            // `[[id]]` naming a block, and an org tag — each needs a context a
            // plain body cannot supply, so each gets a whole file.
            InlineConstruct::LinkById => {
                return Ok(Some(self.survives_in_place(
                    "#+TITLE: Certify\n\n* Target\n:PROPERTIES:\n:ID: \
                     tgt-1\n:END:\n* Head\n:PROPERTIES:\n:ID: h-1\n:END:\nsee \
                     [[tgt-1]]\n",
                    &["[[tgt-1]]"],
                )?));
            }
            InlineConstruct::Tag => {
                return Ok(Some(self.survives_in_place(
                    "#+TITLE: Certify\n\n* Head    :sometag:\n:PROPERTIES:\n:ID: \
                     h-1\n:END:\n",
                    &[":sometag:"],
                )?));
            }
            // escape_sequence is NOT driven, and the honest reason matters: the
            // draft's claim is that backslash escapes are not HONOURED
            // (semantic), while this probe only sees whether the bytes come
            // back. A file whose `\\*` survives verbatim proves nothing about
            // escaping. Certifying it needs an oracle that asks whether the
            // marked-up region was suppressed, which the mark extractor can
            // answer — that is its own piece of work, so the clause stays
            // marked rather than being falsely promoted.
            // link_by_id and tag need an id/tag context the body cannot supply.
            _ => return Ok(None),
        };
        Ok(Some(self.body_survives(body, &[body])?))
    }

    /// Drives `hierarchy.max_depth` — a six-level headline tree.
    fn round_trip_depth(&self, depth: u32) -> anyhow::Result<Option<ConstructOutcome>> {
        let mut src = String::from("#+TITLE: Certify\n\n");
        for level in 1..=depth {
            src.push_str(&format!(
                "{} Level {level}\n:PROPERTIES:\n:ID: lvl-{level}\n:END:\n",
                "*".repeat(level as usize)
            ));
        }
        let deepest = format!("Level {depth}");
        Ok(Some(self.survives_in_place(&src, &[&deepest])?))
    }

    /// Drives `hierarchy.constraints` — each NAMED rule must actually refuse.
    fn violate_constraint(
        &self,
        constraint: ConstraintId,
    ) -> anyhow::Result<Option<ConstructOutcome>> {
        match constraint {
            // NOT DRIVEABLE FROM THIS CRATE, and the reason is structural
            // rather than incidental. The rule is real, but it is enforced ONE
            // LAYER UP: `docs/Reference/ORG_SYNTAX.md:186-191` names the
            // refusal site as `DocumentManager::name_chain`
            // (`crates/holon-filesystem/src/sync_ports.rs`), while this harness
            // calls `parse_org_file` — the FORMAT layer, which never reaches
            // it. A probe here parses the file happily and would report the
            // constraint unenforced, which is false.
            //
            // Reporting `None` is the honest answer: the clause stays MARKED
            // and the coverage law keeps demanding it, rather than a
            // format-layer probe manufacturing a violation against a rule that
            // lives in the sync layer.
            ConstraintId::PageTagRequiresPageAncestor => Ok(None),
            // Not an org rule at all — that one belongs to logseq-db.
            ConstraintId::NoSlashInPageName => Ok(None),
            // Violated by an id carrying a SPACE — the likeliest real typo, and
            // measured to be refused (by panic; see the bug-funnel entry).
            ConstraintId::ValidUriPath => {
                let src = "#+TITLE: Certify\n\n* Head\n:PROPERTIES:\n:ID: has \
                           space\n:END:\n";
                let attempt = std::panic::catch_unwind(|| {
                    parse_org_file(
                        Path::new(FILE),
                        src,
                        &EntityUri::no_parent(),
                        Path::new(ROOT),
                    )
                    .map(|_| ())
                    .map_err(|e| e.to_string())
                });
                Ok(Some(match attempt {
                    Ok(Ok(())) => ConstructOutcome::Survived,
                    Ok(Err(reason)) => ConstructOutcome::Refused { reason },
                    Err(_) => ConstructOutcome::Refused {
                        reason: "PANIC (not a recoverable Err)".to_string(),
                    },
                }))
            }
        }
    }

    /// Drives `hierarchy.cycles` — two blocks each claiming the other as parent
    /// via the `:ID:`/ordering the parser reconstructs.
    fn introduce_cycle(&self) -> anyhow::Result<Option<ConstructOutcome>> {
        // Two headlines sharing ONE id: the parser's cycle/duplicate guard
        // (parser.rs:514-549 reject_id_cycles) is what must speak.
        let src = "#+TITLE: Certify\n\n* First\n:PROPERTIES:\n:ID: same-id\n:END:\n* \
                   Second\n:PROPERTIES:\n:ID: same-id\n:END:\n";
        Ok(Some(self.survives_in_place(src, &["first", "second"])?))
    }

    /// Drives `assets.extensions` — a declared extension must survive, and one
    /// outside the set must not be silently accepted.
    fn round_trip_attachment(&self, ext: &Extension) -> anyhow::Result<Option<ConstructOutcome>> {
        let path = format!("pic.{}", ext.as_str());
        let src = format!(
            "#+TITLE: Certify\n\n* {PROBE_HEADLINE}\n:PROPERTIES:\n:ID: \
             {BLOCK_ID}\n:END:\n[[file:{path}]]\n"
        );
        let parsed = match parse_org_file(
            Path::new(FILE),
            &src,
            &EntityUri::no_parent(),
            Path::new(ROOT),
        ) {
            Ok(p) => p,
            Err(e) => {
                return Ok(Some(ConstructOutcome::Refused {
                    reason: e.to_string(),
                }));
            }
        };

        // The question is whether the link became an ATTACHMENT, not whether
        // its text survived. Every `[[file:…]]` line survives as ordinary body
        // text whatever its extension (`parser.rs:1322-1334` only lifts the
        // ones `is_image_path` recognises), so a text-survival check would
        // report every extension as carried and the clause would certify
        // nothing.
        let became_image = parsed
            .blocks
            .iter()
            .any(|b| b.is_image_block() && b.content == path);
        Ok(Some(if became_image {
            ConstructOutcome::Survived
        } else {
            ConstructOutcome::Lost
        }))
    }

    /// Drives `property_keys.collision` — the SAME key twice in one drawer.
    fn collide_key(&self, first: &Value, second: &Value) -> anyhow::Result<Option<Readback>> {
        let (a, b) = (
            first.as_string().unwrap_or_default(),
            second.as_string().unwrap_or_default(),
        );
        let src = format!(
            "#+TITLE: Certify\n\n* {PROBE_HEADLINE}\n:PROPERTIES:\n:ID: \
             {BLOCK_ID}\n:Dup: {a}\n:Dup: {b}\n:END:\n"
        );
        let parsed = match parse_org_file(
            Path::new(FILE),
            &src,
            &EntityUri::no_parent(),
            Path::new(ROOT),
        ) {
            Ok(p) => p,
            Err(e) => {
                return Ok(Some(Readback::Refused {
                    reason: e.to_string(),
                }));
            }
        };
        let block = parsed
            .blocks
            .iter()
            .find(|b| b.org_title() == PROBE_HEADLINE)
            .ok_or_else(|| anyhow::anyhow!("the collision fixture must carry the probe block"))?;
        Ok(Some(match block.get_property("Dup") {
            Some(v) => Readback::Present(v),
            None => Readback::Absent,
        }))
    }

    /// Drives `identity.id_origin` — an AUTHORED id must survive ingest, or
    /// every inbound link silently detaches.
    fn id_after_ingest(&self, authored_id: &str) -> anyhow::Result<Option<String>> {
        let src = format!(
            "#+TITLE: Certify\n\n* {PROBE_HEADLINE}\n:PROPERTIES:\n:ID: \
             {authored_id}\n:END:\n"
        );
        let parsed = parse_org_file(
            Path::new(FILE),
            &src,
            &EntityUri::no_parent(),
            Path::new(ROOT),
        )
        .map_err(|e| anyhow::anyhow!("the id fixture must parse: {e}"))?;
        let block = parsed
            .blocks
            .iter()
            .find(|b| b.org_title() == PROBE_HEADLINE)
            .ok_or_else(|| anyhow::anyhow!("the id fixture must carry the probe block"))?;
        Ok(Some(block.id.id().to_string()))
    }

    /// Drives `multi_value.separator` AND `.semantics` through the REQUIRES
    /// edge field — the only place org splits at all.
    fn round_trip_multi_value(
        &self,
        values: &[&str],
        separator: &str,
    ) -> anyhow::Result<Option<MultiValueReadback>> {
        let joined = values.join(separator);
        let src = format!(
            "#+TITLE: Certify\n\n* {PROBE_HEADLINE}\n:PROPERTIES:\n:ID: \
             {BLOCK_ID}\n:REQUIRES: {joined}\n:END:\n"
        );
        // A field that did NOT split leaves one value, and one value that is
        // not a bare id is REFUSED — the law's other legal branch, not a
        // harness failure.
        let parsed = match parse_org_file(
            Path::new(FILE),
            &src,
            &EntityUri::no_parent(),
            Path::new(ROOT),
        ) {
            Ok(parsed) => parsed,
            Err(e) => {
                return Ok(Some(MultiValueReadback::Refused {
                    reason: e.to_string(),
                }));
            }
        };
        // FULL round trip, not parse-only: the claim is what survives to disk
        // and back. Reading `requires` straight off the parse would measure
        // the parser's order and miss the renderer's sort — an adjacent
        // measurement wearing the clause's name.
        let rendered = OrgRenderer::render_document(
            &parsed.document,
            &parsed.blocks,
            Path::new(FILE),
            &parsed.document.id,
        );
        let back = parse_org_file(
            Path::new(FILE),
            &rendered,
            &EntityUri::no_parent(),
            Path::new(ROOT),
        )
        .map_err(|e| anyhow::anyhow!("the written multi-value must parse back: {e}"))?;
        let block = back
            .blocks
            .iter()
            .find(|b| b.org_title() == PROBE_HEADLINE)
            .ok_or_else(|| anyhow::anyhow!("the multi-value fixture must carry the probe block"))?;
        Ok(Some(MultiValueReadback::Values(
            block.requires.iter().map(|u| u.id().to_string()).collect(),
        )))
    }

    /// Drives `reference_values` through `:REQUIRES:`, org's only
    /// reference-typed drawer key (`parser.rs:1486-1496`).
    ///
    /// The typed readback is `block.requires: Vec<EntityUri>` — a value that
    /// became a reference is IN it, one that did not is a flat string property,
    /// and one the boundary rejected is an `Err`. Those are exactly the three
    /// answers the clause turns on.
    fn round_trip_reference(&self, value: &str) -> anyhow::Result<Option<ReferenceReadback>> {
        let src = format!(
            "#+TITLE: Certify\n\n* {PROBE_HEADLINE}\n:PROPERTIES:\n:ID: \
             {BLOCK_ID}\n:REQUIRES: {value}\n:END:\n"
        );
        let parsed = match parse_org_file(
            Path::new(FILE),
            &src,
            &EntityUri::no_parent(),
            Path::new(ROOT),
        ) {
            Ok(parsed) => parsed,
            Err(e) => {
                return Ok(Some(ReferenceReadback::Refused {
                    reason: e.to_string(),
                }));
            }
        };
        let rendered = OrgRenderer::render_document(
            &parsed.document,
            &parsed.blocks,
            Path::new(FILE),
            &parsed.document.id,
        );
        let back = match parse_org_file(
            Path::new(FILE),
            &rendered,
            &EntityUri::no_parent(),
            Path::new(ROOT),
        ) {
            Ok(back) => back,
            Err(e) => {
                return Ok(Some(ReferenceReadback::Refused {
                    reason: e.to_string(),
                }));
            }
        };
        let block = back
            .blocks
            .iter()
            .find(|b| b.org_title() == PROBE_HEADLINE)
            .ok_or_else(|| anyhow::anyhow!("the reference fixture must carry the probe block"))?;
        if block.requires.is_empty() {
            return Ok(Some(ReferenceReadback::Plain(
                block
                    .properties
                    .get("REQUIRES")
                    .and_then(|v| v.as_string())
                    .unwrap_or_default()
                    .to_string(),
            )));
        }
        Ok(Some(ReferenceReadback::Refs(
            block.requires.iter().map(|u| u.id().to_string()).collect(),
        )))
    }

    /// Drives `identity.carriers` — each carrier in the CLOSED vocabulary.
    fn identity_via(&self, carrier: IdCarrier) -> anyhow::Result<Option<bool>> {
        let (src, wanted): (String, &str) = match carrier {
            IdCarrier::DrawerId => (
                format!(
                    "#+TITLE: Certify\n\n* {PROBE_HEADLINE}\n:PROPERTIES:\n:ID: \
                     drawer-carried\n:END:\n"
                ),
                "drawer-carried",
            ),
            IdCarrier::FileKeywordId => (
                "#+ID: keyword-carried\n#+TITLE: Certify\n\n* Probe headline\n".to_string(),
                "keyword-carried",
            ),
            // MEASURED, and it REFUTES my earlier removal: with NO `:ID:` and no
            // `#+ID:`, the document id is derived from the FILE PATH plus the
            // vault root — both of which this probe already receives. It is a
            // real, format-visible carrier, and deleting it to clear a gap was
            // the wrong move.
            IdCarrier::PathDerived => (
                "#+TITLE: Certify\n\n* Probe headline\n".to_string(),
                "profile",
            ),
            // LogSeq's carriers; not org's at all.
            IdCarrier::NameChain | IdCarrier::BlockUuid | IdCarrier::BlockName => {
                return Ok(None);
            }
        };
        let parsed = parse_org_file(
            Path::new(FILE),
            &src,
            &EntityUri::no_parent(),
            Path::new(ROOT),
        )
        .map_err(|e| anyhow::anyhow!("the carrier fixture must parse: {e}"))?;
        let found = match carrier {
            IdCarrier::DrawerId => parsed.blocks.iter().any(|b| b.id.id().contains(wanted)),
            _ => parsed.document.id.id().contains(wanted),
        };
        Ok(Some(found))
    }

    /// Drives `identity.carrier_disagreement`: a file whose `#+ID:` keyword and

    /// Drives `ordering.sibling_order` — authored file order must come back.
    fn round_trip_sibling_order(&self, authored: &[&str]) -> anyhow::Result<Option<Vec<String>>> {
        let mut src = String::from("#+TITLE: Certify\n\n");
        for (i, title) in authored.iter().enumerate() {
            src.push_str(&format!("* {title}\n:PROPERTIES:\n:ID: sib-{i}\n:END:\n"));
        }
        let parsed = parse_org_file(
            Path::new(FILE),
            &src,
            &EntityUri::no_parent(),
            Path::new(ROOT),
        )
        .map_err(|e| anyhow::anyhow!("the sibling fixture must parse: {e}"))?;
        Ok(Some(
            parsed
                .blocks
                .iter()
                .map(|b| b.org_title())
                .filter(|t| authored.contains(&t.as_str()))
                .collect(),
        ))
    }

    /// Drives `hosted_kinds` from what ingest YIELDS. Org's parse result is
    /// Block-only (`crates/holon-core/src/file_format.rs:26-35`), so every
    /// entity it produces has a place in a tree and a free-standing typed row
    /// has no org representation at all.
    fn all_entities_hierarchical(&self) -> anyhow::Result<Option<bool>> {
        let parsed = parse_org_file(
            Path::new(FILE),
            BASE_FIXTURE,
            &EntityUri::no_parent(),
            Path::new(ROOT),
        )
        .map_err(|e| anyhow::anyhow!("the base fixture must parse: {e}"))?;
        // Every parsed block names a parent; nothing free-standing can come out.
        Ok(Some(
            parsed.blocks.iter().all(|b| !b.parent_id.id().is_empty()),
        ))
    }

    /// Drives `content.representation` — a marked span must yield MARK DATA.
    fn marks_are_parsed(&self) -> anyhow::Result<Option<bool>> {
        let src = format!(
            "#+TITLE: Certify\n\n* {PROBE_HEADLINE}\n:PROPERTIES:\n:ID: \
             {BLOCK_ID}\n:END:\nsome *bold* text\n"
        );
        let parsed = parse_org_file(
            Path::new(FILE),
            &src,
            &EntityUri::no_parent(),
            Path::new(ROOT),
        )
        .map_err(|e| anyhow::anyhow!("the marks fixture must parse: {e}"))?;
        // Marks anywhere in the document: the claim is that the format PARSES
        // markup, not that a particular block carries it.
        Ok(Some(parsed.blocks.iter().any(|b| b.marks.is_some())))
    }

    /// Drives `ordering.order_key_durable` — `derived` claims NO explicit order
    /// key reaches disk.
    fn writes_explicit_order_key(&self) -> anyhow::Result<Option<bool>> {
        let mut src = String::from("#+TITLE: Certify\n\n");
        for i in 0..3 {
            src.push_str(&format!("* Sib {i}\n:PROPERTIES:\n:ID: k-{i}\n:END:\n"));
        }
        let parsed = parse_org_file(
            Path::new(FILE),
            &src,
            &EntityUri::no_parent(),
            Path::new(ROOT),
        )
        .map_err(|e| anyhow::anyhow!("the order-key fixture must parse: {e}"))?;
        let rendered = OrgRenderer::render_document(
            &parsed.document,
            &parsed.blocks,
            Path::new(FILE),
            &parsed.document.id,
        )
        .to_lowercase();
        // Any of the shapes an explicit key could take on disk.
        Ok(Some(
            rendered.contains(":order:")
                || rendered.contains(":sort_key:")
                || rendered.contains(":sequence:"),
        ))
    }

    /// Drives `hierarchy.shape` — `forest` claims several roots coexist.
    fn holds_multiple_roots(&self) -> anyhow::Result<Option<bool>> {
        let src = "#+TITLE: Certify\n\n* Root A\n:PROPERTIES:\n:ID: r-a\n:END:\n* Root \
                   B\n:PROPERTIES:\n:ID: r-b\n:END:\n";
        let parsed = parse_org_file(
            Path::new(FILE),
            src,
            &EntityUri::no_parent(),
            Path::new(ROOT),
        )
        .map_err(|e| anyhow::anyhow!("the forest fixture must parse: {e}"))?;
        let roots = parsed
            .blocks
            .iter()
            .filter(|b| b.parent_id == parsed.document.id)
            .count();
        Ok(Some(roots >= 2))
    }

    /// Drives `identity.id_space` / `identity.id_constraints` with HOSTILE ids.
    /// An empty constraint list is a claim that NONE of these is refused.
    fn id_refused(&self, id: &str) -> anyhow::Result<Option<Option<String>>> {
        let src =
            format!("#+TITLE: Certify\n\n* {PROBE_HEADLINE}\n:PROPERTIES:\n:ID: {id}\n:END:\n");
        // `catch_unwind` because a refusal here may arrive as a PANIC rather
        // than an `Err`: `EntityUri::from_raw` parses via fluent_uri and
        // panics on a path it cannot form. A panic IS a refusal in effect, but
        // an unrecoverable one — the distinction is reported, not smoothed
        // over, because crashing on a hand-authored file is a different
        // severity from refusing it.
        let attempt = std::panic::catch_unwind(|| {
            parse_org_file(
                Path::new(FILE),
                &src,
                &EntityUri::no_parent(),
                Path::new(ROOT),
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
        });
        Ok(Some(match attempt {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e),
            Err(_) => Some("PANIC (not a recoverable Err)".to_string()),
        }))
    }

    /// Drives `mutation.unit_of_write` — the falsifier is a PARTIAL write.
    ///
    /// The previous version rendered the SAME argument twice and asserted the
    /// results equal, which is X==X and cannot fail. The real question is
    /// whether a one-block change produces the WHOLE document: render after
    /// touching one block, then render ONLY that block via `render_blocks`
    /// (the fragment API that exists), and require that the write path's output
    /// is the whole-document form — strictly longer than the fragment, and
    /// carrying the UNTOUCHED sibling's bytes.
    fn single_change_emits_whole_document(&self) -> anyhow::Result<Option<bool>> {
        let src = "#+TITLE: Certify\n\n* Alpha\n:PROPERTIES:\n:ID: w-a\n:END:\n* \
                   Beta\n:PROPERTIES:\n:ID: w-b\n:END:\n";
        let parsed = parse_org_file(
            Path::new(FILE),
            src,
            &EntityUri::no_parent(),
            Path::new(ROOT),
        )
        .map_err(|e| anyhow::anyhow!("the write-unit fixture must parse: {e}"))?;

        let mut changed = parsed.blocks.clone();
        let target = changed
            .iter_mut()
            .find(|b| b.org_title() == "Alpha")
            .ok_or_else(|| anyhow::anyhow!("the write-unit fixture must carry Alpha"))?;
        target.set_property("Touched", Value::String("yes".to_string()));

        let whole = OrgRenderer::render_document(
            &parsed.document,
            &changed,
            Path::new(FILE),
            &parsed.document.id,
        );
        // The FRAGMENT form: just the block that changed.
        let only_changed: Vec<_> = changed
            .iter()
            .filter(|b| b.org_title() == "Alpha")
            .cloned()
            .collect();
        // `render_entitys` is the fragment API that exists: blocks only, no
        // document header.
        let fragment =
            OrgRenderer::render_entitys(&only_changed, Path::new(FILE), &parsed.document.id);

        Ok(Some(
            // whole-document: carries the UNTOUCHED sibling, carries the
            // document header, and is strictly bigger than the fragment.
            whole.contains("Beta")
                && whole.contains("#+TITLE:")
                && whole.len() > fragment.len()
                && !fragment.contains("Beta"),
        ))
    }

    /// Drives `mutation.write_leg` by ATTEMPTING A WRITE through the real path.
    ///
    /// Asked of the FORMAT: the old probe compared `write_leg` against
    /// `supports()`, which derives from `write_leg`, so it passed for any
    /// declaration — including declaring this writable format read-only.
    fn attempt_write(&self) -> anyhow::Result<Option<WriteAttempt>> {
        let parsed = parse_org_file(
            Path::new(FILE),
            BASE_FIXTURE,
            &EntityUri::no_parent(),
            Path::new(ROOT),
        )
        .map_err(|e| anyhow::anyhow!("the write fixture must parse: {e}"))?;
        let mut blocks = parsed.blocks.clone();
        let target = blocks
            .iter_mut()
            .find(|b| b.org_title() == PROBE_HEADLINE)
            .ok_or_else(|| anyhow::anyhow!("the write fixture must carry the probe block"))?;
        target.set_property("WrittenBy", Value::String("certifier".to_string()));

        let rendered = OrgRenderer::render_document(
            &parsed.document,
            &blocks,
            Path::new(FILE),
            &parsed.document.id,
        );
        // A write COUNTS only if it round-trips: bytes produced AND read back.
        let back = parse_org_file(
            Path::new(FILE),
            &rendered,
            &EntityUri::no_parent(),
            Path::new(ROOT),
        )
        .map_err(|e| anyhow::anyhow!("the written bytes must parse back: {e}"))?;
        let survived = back
            .blocks
            .iter()
            .any(|b| b.get_property("WrittenBy").is_some());
        Ok(Some(if survived {
            // The mechanism is FILE bytes: `render_document` returns the whole
            // file's text, which is what the caller writes to disk. Naming the
            // leg is what lets `write_leg` answer "which", not just "whether".
            WriteAttempt::Wrote {
                leg: WriteLeg::File,
            }
        } else {
            WriteAttempt::Refused {
                reason: "the write produced bytes but the value did not return".to_string(),
            }
        }))
    }

    /// Drives `assets.attachments` / `assets.binary_inline` — an attachment is
    /// carried as a PATH REFERENCE, never as embedded bytes.
    fn attachment_is_reference(&self) -> anyhow::Result<Option<bool>> {
        let src = format!(
            "#+TITLE: Certify\n\n* {PROBE_HEADLINE}\n:PROPERTIES:\n:ID: \
             {BLOCK_ID}\n:END:\n[[file:pic.png]]\n"
        );
        let parsed = parse_org_file(
            Path::new(FILE),
            &src,
            &EntityUri::no_parent(),
            Path::new(ROOT),
        )
        .map_err(|e| anyhow::anyhow!("the attachment fixture must parse: {e}"))?;
        let image = parsed.blocks.iter().find(|b| b.is_image_block());
        Ok(Some(match image {
            // A reference: the block carries the PATH, short and pointing
            // outward. Embedded bytes would be neither.
            Some(b) => b.content == "pic.png",
            None => false,
        }))
    }

    /// Drives `identity.carrier_disagreement`: a file whose `#+ID:` keyword and
    /// whose file-level `:ID:` drawer name DIFFERENT identities
    /// (`docs/Reference/ORG_SYNTAX.md:79-84`).
    fn carriers_disagree(&self) -> anyhow::Result<Option<DisagreementOutcome>> {
        let path = Path::new(FILE);
        // POSITION IS PART OF THE GRAMMAR: a file-level drawer is only
        // recognised as the FIRST element of the file
        // (`docs/Reference/ORG_SYNTAX.md:72-75`). Putting `#+TITLE:` above it
        // makes the drawer ordinary text, and then the two carriers never
        // actually disagree — the probe would be measuring nothing while
        // looking like a finding.
        let src = ":PROPERTIES:\n:ID: drawer-identity\n:END:\n#+ID: keyword-identity\n#+TITLE: \
                   Certify\n\n* Probe headline\n";

        Ok(Some(
            match parse_org_file(path, src, &EntityUri::no_parent(), Path::new(ROOT)) {
                Err(e) => DisagreementOutcome::Refused {
                    reason: e.to_string(),
                },
                // Parsed anyway: SOMETHING was chosen, and which carrier won is
                // what the profile must then declare.
                Ok(parsed) => {
                    let id = parsed.document.id.id().to_string();
                    DisagreementOutcome::Picked {
                        carrier: if id.contains("drawer-identity") {
                            IdCarrier::DrawerId
                        } else {
                            IdCarrier::FileKeywordId
                        },
                    }
                }
            },
        ))
    }

    /// Attach a tag to the structured set and read it back off the RE-PARSED
    /// block.
    fn attach_existing_tag(&self) -> anyhow::Result<Option<ConstructOutcome>> {
        let back = self.tags_round_trip(&["keep"], &["keep", "added"])?;
        Ok(Some(if back.contains("added") && back.contains("keep") {
            ConstructOutcome::Survived
        } else if back.contains("added") {
            ConstructOutcome::Changed {
                got: format!("{back:?}"),
            }
        } else {
            ConstructOutcome::Lost
        }))
    }

    /// Remove one tag of two and require the removal — and only it — to land.
    fn detach_existing_tag(&self) -> anyhow::Result<Option<ConstructOutcome>> {
        let back = self.tags_round_trip(&["keep", "drop"], &["keep"])?;
        Ok(Some(if !back.contains("drop") && back.contains("keep") {
            ConstructOutcome::Survived
        } else if !back.contains("drop") {
            // The tag went, and so did the one that should have stayed: a
            // detach that empties the set is not a detach.
            ConstructOutcome::Changed {
                got: format!("{back:?}"),
            }
        } else {
            ConstructOutcome::Lost
        }))
    }

    /// Write a tag name nothing in the file has used before.
    ///
    /// Org has no tag ENTITY, so there is nothing for a name to fail to
    /// resolve to and nothing to dangle: the two observable answers are
    /// REFUSED and carried-into-existence, and this probe distinguishes them.
    /// `minted` is the honest reading of the second — the reference is what
    /// brings the tag into being.
    fn reference_unknown_tag(&self) -> anyhow::Result<Option<ConstructOutcome>> {
        let back = self.tags_round_trip(&["keep"], &["keep", "neverseenbefore"])?;
        Ok(Some(if back.contains("neverseenbefore") {
            ConstructOutcome::Survived
        } else {
            ConstructOutcome::Lost
        }))
    }
}

/// The whole increment in one assertion: every clause the org profile declares
/// is REAL.
///
/// A red here means either the adapter or the profile is lying, and the
/// `Violation` payload names which axis, which clause, which leg and which
/// value — enough to act on without a debugger.
#[test]
fn the_org_profile_declares_only_restrictions_that_are_real() {
    let format = OrgFormat::load();
    let report = certify(&format).expect("the certification harness must run");

    println!("{}", report.render());

    // The run's machine-readable half. Written under `target/` — never under
    // `docs/` — so a test can never dirty the source tree. A human turns it
    // into ledger entries with `scripts/capability-ledger.py sync`.
    //
    // `HOLON_CAPABILITY_REPORT_DIR` sends it elsewhere. The flip sweep uses
    // that: a sweep run writing here would leave the ledger's input describing
    // a deliberately broken profile, and `capability-ledger.py diff` would then
    // accuse the honest profile of a prompt no honest run raises.
    let dir = std::env::var_os("HOLON_CAPABILITY_REPORT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/capability-certification")
        });
    let written = report
        .write_report(format.profile().id(), &dir)
        .expect("the certification report must be writable");
    println!("report: {}", written.display());

    assert!(
        report.confirmed > 0,
        "a run that generated NOTHING must not pass as clean:\n{}",
        report.render()
    );
    assert!(
        report.is_clean(),
        "the org profile declares {} restriction(s) the format does not honour:\n{}",
        report.violations.len(),
        report.render()
    );
}

/// The CONTROL, and it must hold under a LYING profile too.
///
/// A certifier that fails on everything proves nothing. This pins the other
/// side of the discrimination: an ordinary string property on an unreserved
/// key survives both legs intact, so a red in the test above is attributable
/// to the clause it names rather than to a broken harness.
#[test]
fn a_plain_string_property_survives_both_legs() {
    let format = OrgFormat::load();
    let sent = Value::String("carried".to_string());

    for carrier in format.carriers() {
        let back = format
            .round_trip_property(*carrier, "Plain", &sent)
            .expect("the harness must run");
        assert_eq!(
            back,
            Readback::Present(sent.clone()),
            "the control must survive the {} leg — if it does not, the harness is broken and \
             every other verdict in this file is worthless",
            carrier.leg,
        );
    }
}
