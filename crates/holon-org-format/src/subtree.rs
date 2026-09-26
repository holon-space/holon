//! Headline subtrees of org text as byte ranges.
//!
//! A headline's subtree is contiguous: from its headline line to the next
//! headline of the same or a higher level. A headline is identified by the
//! `:ID:` line of its own section (the text before its first child).

use std::ops::Range;

use anyhow::Result;

/// One headline line of an org text.
#[derive(Debug, Clone)]
struct Headline {
    /// Byte offset of the headline line.
    start: usize,
    /// Byte offset where its own section ends: the next headline of any level.
    section_end: usize,
    level: usize,
    id: Option<String>,
}

fn headline_level(line: &str) -> Option<usize> {
    let stars = line.bytes().take_while(|b| *b == b'*').count();
    if stars == 0 {
        return None;
    }
    match line.as_bytes().get(stars) {
        None | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') => Some(stars),
        _ => None,
    }
}

fn section_id(section: &str) -> Option<String> {
    section.lines().skip(1).find_map(|line| {
        let line = line.trim();
        let (key, value) = line.split_at_checked(4)?;
        (key.eq_ignore_ascii_case(":ID:") && !value.trim().is_empty())
            .then(|| value.trim().to_string())
    })
}

fn headlines(text: &str) -> Vec<Headline> {
    let mut starts: Vec<(usize, usize)> = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if let Some(level) = headline_level(line) {
            starts.push((offset, level));
        }
        offset += line.len();
    }
    let mut out = Vec::with_capacity(starts.len());
    for (i, &(start, level)) in starts.iter().enumerate() {
        let section_end = starts.get(i + 1).map_or(text.len(), |(next, _)| *next);
        out.push(Headline {
            start,
            section_end,
            level,
            id: section_id(&text[start..section_end]),
        });
    }
    out
}

fn subtree_end(all: &[Headline], index: usize, text_len: usize) -> usize {
    let level = all[index].level;
    all[index + 1..]
        .iter()
        .find(|h| h.level <= level)
        .map_or(text_len, |h| h.start)
}

/// The byte range of the subtree whose headline carries `:ID: <bare_id>`.
pub fn subtree_range(text: &str, bare_id: &str) -> Option<Range<usize>> {
    let all = headlines(text);
    let index = all.iter().position(|h| h.id.as_deref() == Some(bare_id))?;
    Some(all[index].start..subtree_end(&all, index, text.len()))
}

/// The level of the headline that carries `:ID: <bare_id>`.
pub fn headline_level_of(text: &str, bare_id: &str) -> Option<usize> {
    headlines(text)
        .into_iter()
        .find(|h| h.id.as_deref() == Some(bare_id))
        .map(|h| h.level)
}

/// `section` with every headline shifted so its first headline is at `level`.
pub fn relevel(section: &str, level: usize) -> String {
    assert!(level >= 1, "an org headline has at least one star");
    let top = section
        .lines()
        .next()
        .and_then(headline_level)
        .unwrap_or_else(|| panic!("a subtree starts with a headline: {section:?}"));
    section
        .split_inclusive('\n')
        .map(|line| match headline_level(line) {
            Some(stars) => {
                let shifted = (stars + level)
                    .checked_sub(top)
                    .filter(|s| *s >= 1)
                    .unwrap_or_else(|| {
                        panic!("headline {line:?} is above the subtree's own headline")
                    });
                format!("{}{}", "*".repeat(shifted), &line[stars..])
            }
            None => line.to_string(),
        })
        .collect()
}

fn with_newline(text: &str) -> String {
    if text.is_empty() || text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    }
}

/// `text` without the subtree of `bare_id`, and that subtree.
pub fn cut_subtree(text: &str, bare_id: &str) -> Option<(String, String)> {
    let range = subtree_range(text, bare_id)?;
    let section = with_newline(&text[range.clone()]);
    let mut rest = text[..range.start].to_string();
    rest.push_str(&text[range.end..]);
    Some((rest, section))
}

/// `text` with `section` appended as a top-level subtree.
pub fn append_subtree(text: &str, section: &str) -> String {
    let mut out = with_newline(text);
    out.push_str(&with_newline(&relevel(section, 1)));
    out
}

/// `text` with `section` inserted as the first top-level subtree, after the
/// text before the first headline.
pub fn prepend_subtree(text: &str, section: &str) -> String {
    let at = headlines(text).first().map_or(text.len(), |h| h.start);
    let mut out = with_newline(&text[..at]);
    out.push_str(&with_newline(&relevel(section, 1)));
    out.push_str(&text[at..]);
    out
}

/// `text` with the headline of `bare_id` carrying the task keyword `keyword`
/// (none when empty) instead of whichever of `keywords` it carries.
pub fn set_headline_keyword(
    text: &str,
    bare_id: &str,
    keyword: &str,
    keywords: &[&str],
) -> Option<String> {
    let range = subtree_range(text, bare_id)?;
    let line_end = text[range.start..]
        .find('\n')
        .map_or(text.len(), |i| range.start + i);
    let line = &text[range.start..line_end];
    let stars = line.bytes().take_while(|b| *b == b'*').count();
    let rest = line[stars..].trim_start();
    let title = match rest.split_once(' ') {
        Some((first, after)) if keywords.contains(&first) => after,
        None if keywords.contains(&rest) => "",
        _ => rest,
    };
    let headline = if keyword.is_empty() {
        format!("{} {title}", "*".repeat(stars))
    } else {
        format!("{} {keyword} {title}", "*".repeat(stars))
    };
    let mut out = text[..range.start].to_string();
    out.push_str(headline.trim_end());
    out.push_str(&text[line_end..]);
    Some(out)
}

/// `rendered` with the subtree of each id in `bare_ids` inserted as `disk`
/// holds it, at the position `disk` gives it:
///
/// 1. after the subtree of its nearest previous sibling on `disk` that
///    `rendered` holds, at that sibling's level;
/// 2. else directly after the own section of its nearest ancestor on `disk`
///    that `rendered` holds, one level below it;
/// 3. else before the first headline, at level 1.
///
/// Only the headline stars of a kept subtree change, and only when its
/// anchor's level differs from its level on `disk`. An id `disk` does not
/// hold is skipped. Ids are placed in `disk` order, so one kept subtree can
/// anchor another.
pub fn keep_subtrees(disk: &str, rendered: &str, bare_ids: &[String]) -> Result<String> {
    let on_disk = headlines(disk);
    let mut kept: Vec<usize> = bare_ids
        .iter()
        .filter_map(|id| on_disk.iter().position(|h| h.id.as_deref() == Some(id)))
        .collect();
    kept.sort_unstable();
    kept.dedup();
    let mut out = rendered.to_string();
    for index in kept {
        let own = &on_disk[index];
        let section = with_newline(&disk[own.start..subtree_end(&on_disk, index, disk.len())]);
        let placed = headlines(&out);
        let find = |id: &Option<String>| {
            id.as_ref()
                .and_then(|id| placed.iter().position(|h| h.id.as_ref() == Some(id)))
        };
        let mut anchor: Option<(usize, usize)> = None;
        let mut min_level = own.level;
        for earlier in on_disk[..index].iter().rev() {
            if earlier.level < min_level {
                min_level = earlier.level;
                if let Some(at) = find(&earlier.id) {
                    anchor = Some((placed[at].section_end, placed[at].level + 1));
                    break;
                }
            } else if earlier.level == own.level && min_level == own.level {
                if let Some(at) = find(&earlier.id) {
                    anchor = Some((subtree_end(&placed, at, out.len()), placed[at].level));
                    break;
                }
            }
        }
        let (at, level) =
            anchor.unwrap_or_else(|| (placed.first().map_or(out.len(), |h| h.start), 1));
        let mut spliced = with_newline(&out[..at]);
        spliced.push_str(&relevel(&section, level));
        spliced.push_str(&out[at..]);
        out = spliced;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISK: &str = "#+ID: page\n\
* A\n:PROPERTIES:\n:ID: a\n:END:\n\
* X\n:PROPERTIES:\n:ID: x\n:END:\nbody of x\n** X child\n:PROPERTIES:\n:ID: xc\n:END:\n\
* B\n:PROPERTIES:\n:ID: b\n:END:\n";

    #[test]
    fn a_subtree_ends_at_the_next_headline_of_its_level() {
        let range = subtree_range(DISK, "x").unwrap();
        assert!(DISK[range.clone()].starts_with("* X\n"));
        assert!(DISK[range].ends_with(":ID: xc\n:END:\n"));
    }

    #[test]
    fn a_kept_subtree_follows_its_previous_sibling() {
        let rendered = "#+ID: page\n* A edited\n:PROPERTIES:\n:ID: a\n:END:\n\
* New\n:PROPERTIES:\n:ID: n\n:END:\n* B\n:PROPERTIES:\n:ID: b\n:END:\n";
        let out = keep_subtrees(DISK, rendered, &["x".to_string()]).unwrap();
        let x = out.find("* X\n").unwrap();
        assert!(out.find("* A edited").unwrap() < x && x < out.find("* New").unwrap());
        assert!(out.contains("body of x\n** X child\n"));
    }

    #[test]
    fn a_first_child_copy_goes_right_after_its_parent_section_at_its_new_level() {
        let disk = "* P\n:PROPERTIES:\n:ID: p\n:END:\n** X\n:PROPERTIES:\n:ID: x\n:END:\n\
** C\n:PROPERTIES:\n:ID: c\n:END:\n";
        let rendered = "* Q\n:PROPERTIES:\n:ID: q\n:END:\n** P\n:PROPERTIES:\n:ID: p\n:END:\n\
*** C\n:PROPERTIES:\n:ID: c\n:END:\n";
        let out = keep_subtrees(disk, rendered, &["x".to_string()]).unwrap();
        assert_eq!(
            out,
            "* Q\n:PROPERTIES:\n:ID: q\n:END:\n** P\n:PROPERTIES:\n:ID: p\n:END:\n\
*** X\n:PROPERTIES:\n:ID: x\n:END:\n*** C\n:PROPERTIES:\n:ID: c\n:END:\n"
        );
    }

    #[test]
    fn without_any_anchor_a_copy_goes_before_the_first_headline() {
        let rendered = "#+ID: page\n* B\n:PROPERTIES:\n:ID: b\n:END:\n";
        let disk =
            "#+ID: page\n* X\n:PROPERTIES:\n:ID: x\n:END:\n* B\n:PROPERTIES:\n:ID: b\n:END:\n";
        assert_eq!(
            keep_subtrees(disk, rendered, &["x".to_string()]).unwrap(),
            disk
        );
    }

    #[test]
    fn a_prepended_subtree_follows_the_file_header() {
        let out = prepend_subtree("#+ID: page\n* B\n", "** X\n*** Y\n");
        assert_eq!(out, "#+ID: page\n* X\n** Y\n* B\n");
    }

    #[test]
    fn a_headline_keyword_is_replaced_or_removed() {
        let text = "* TODO X\n:PROPERTIES:\n:ID: x\n:END:\n";
        let keywords = ["TODO", "DOING", "DONE"];
        assert_eq!(
            set_headline_keyword(text, "x", "DONE", &keywords).unwrap(),
            "* DONE X\n:PROPERTIES:\n:ID: x\n:END:\n"
        );
        assert_eq!(
            set_headline_keyword(text, "x", "", &keywords).unwrap(),
            "* X\n:PROPERTIES:\n:ID: x\n:END:\n"
        );
    }

    #[test]
    fn cut_then_append_moves_a_subtree_to_the_end_at_level_one() {
        let (rest, section) = cut_subtree(DISK, "x").unwrap();
        assert!(!rest.contains(":ID: x\n"));
        let moved = append_subtree("#+ID: other\n", &section);
        assert!(moved.ends_with("* X\n:PROPERTIES:\n:ID: x\n:END:\nbody of x\n** X child\n:PROPERTIES:\n:ID: xc\n:END:\n"));
    }
}
