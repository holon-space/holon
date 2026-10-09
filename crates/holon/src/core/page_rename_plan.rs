//! The typed plan the `page_rename_plan` read op hands the engine when a
//! page's title changes, and the link rewrite the engine applies to each
//! block it names (D-link-follows-rename: a page rename is a rename
//! refactoring of every link to the page).

use holon_api::EntityRef;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::Value;

/// A block whose name links resolve to the renamed page, with the link
/// targets (as `block_links.target` holds them) that do.
#[derive(Debug, Clone, PartialEq)]
pub struct Backlink {
    pub source: String,
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PageRenamePlan {
    pub backlinks: Vec<Backlink>,
}

impl PageRenamePlan {
    pub fn to_value(&self) -> Value {
        Value::Array(
            self.backlinks
                .iter()
                .map(|b| {
                    let mut obj = std::collections::HashMap::new();
                    obj.insert("source".to_string(), Value::String(b.source.clone()));
                    obj.insert(
                        "targets".to_string(),
                        Value::Array(b.targets.iter().cloned().map(Value::String).collect()),
                    );
                    Value::Object(obj)
                })
                .collect(),
        )
    }

    pub fn from_value(value: &Value) -> Result<Self, String> {
        let Value::Array(items) = value else {
            return Err(format!("PageRenamePlan: expected Array, got {value:?}"));
        };
        let backlinks = items
            .iter()
            .map(|item| {
                let Value::Object(obj) = item else {
                    return Err(format!("PageRenamePlan: expected Object, got {item:?}"));
                };
                let source = obj
                    .get("source")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| format!("PageRenamePlan: backlink without 'source': {obj:?}"))?
                    .to_string();
                let Some(Value::Array(targets)) = obj.get("targets") else {
                    return Err(format!(
                        "PageRenamePlan: backlink without 'targets': {obj:?}"
                    ));
                };
                let targets = targets
                    .iter()
                    .map(|t| {
                        t.as_string().map(str::to_string).ok_or_else(|| {
                            format!("PageRenamePlan: non-string target {t:?} of {source}")
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Backlink { source, targets })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self { backlinks })
    }
}

/// Rewrite every name link in `(content, marks)` whose target is one of
/// `targets` to name `new_title` (the leaf of a `parent/leaf` chain). A bare
/// link (label == target) shows the new name; an explicit label is prose and
/// stays as authored.
pub fn rewrite_name_links(
    content: &str,
    marks: &[MarkSpan],
    targets: &[String],
    new_title: &str,
) -> (String, Vec<MarkSpan>) {
    let mut chars: Vec<char> = content.chars().collect();
    let mut marks = marks.to_vec();
    let mut hits: Vec<usize> = marks
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            matches!(&m.mark, InlineMark::Link { target: EntityRef::Name { name }, .. }
                if targets.contains(name))
        })
        .map(|(i, _)| i)
        .collect();
    hits.sort_by_key(|&i| std::cmp::Reverse(marks[i].start));
    for i in hits {
        let (start, end) = (marks[i].start, marks[i].end);
        let InlineMark::Link {
            target: EntityRef::Name { name },
            label,
        } = &mut marks[i].mark
        else {
            unreachable!("hit {i} is a name link");
        };
        let renamed = match name.rsplit_once('/') {
            Some((parent, _)) => format!("{parent}/{new_title}"),
            None => new_title.to_string(),
        };
        let bare = label == name;
        *name = renamed.clone();
        if !bare {
            continue;
        }
        *label = renamed.clone();
        chars.splice(start..end, renamed.chars());
        let new_end = start + renamed.chars().count();
        let shift = |p: usize| if p >= end { p - end + new_end } else { p };
        for (j, m) in marks.iter_mut().enumerate() {
            if j == i {
                m.end = new_end;
            } else {
                m.start = shift(m.start);
                m.end = shift(m.end);
            }
        }
    }
    (chars.into_iter().collect(), marks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(start: usize, end: usize, name: &str, label: &str) -> MarkSpan {
        MarkSpan::new(
            start,
            end,
            InlineMark::Link {
                target: EntityRef::Name { name: name.into() },
                label: label.into(),
            },
        )
    }

    #[test]
    fn a_bare_link_shows_the_new_name_and_later_marks_shift() {
        let marks = vec![
            link(4, 9, "pagea", "pagea"),
            MarkSpan::new(10, 13, InlineMark::Bold),
        ];
        let (content, marks) =
            rewrite_name_links("see pagea now", &marks, &["pagea".into()], "Renamed");
        assert_eq!(content, "see Renamed now");
        assert_eq!(
            marks,
            vec![
                link(4, 11, "Renamed", "Renamed"),
                MarkSpan::new(12, 15, InlineMark::Bold)
            ]
        );
    }

    #[test]
    fn an_explicit_label_stays_and_a_chain_keeps_its_parent() {
        let marks = vec![
            link(0, 8, "P/pagea", "see here"),
            link(9, 16, "P/pagea", "P/pagea"),
        ];
        let (content, marks) =
            rewrite_name_links("see here P/pagea", &marks, &["P/pagea".into()], "B");
        assert_eq!(content, "see here P/B");
        assert_eq!(
            marks,
            vec![link(0, 8, "P/B", "see here"), link(9, 12, "P/B", "P/B")]
        );
    }

    #[test]
    fn a_link_to_another_page_is_untouched() {
        let marks = vec![link(0, 5, "other", "other")];
        let (content, out) = rewrite_name_links("other", &marks, &["pagea".into()], "B");
        assert_eq!((content.as_str(), out), ("other", marks));
    }
}
