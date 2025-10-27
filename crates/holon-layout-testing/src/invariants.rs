//! Layout invariant functions. All take a `&BoundsSnapshot` and a fixture
//! label and panic (or return `Err`) on violation.

use crate::snapshot::{BoundsSnapshot, Rect, VISIBLE_LEAF_TYPES};
use holon_frontend::geometry::ElementInfo;

/// Fail if the snapshot is empty — catches "nothing rendered at all" bugs.
pub fn assert_nonempty(snap: &BoundsSnapshot, fixture_label: &str) {
    assert!(
        !snap.is_empty(),
        "fixture `{fixture_label}` produced an empty BoundsSnapshot — no widgets were tracked. \
         Either nothing rendered, or `tag()` isn't wired through `builder_registry!`.",
    );
}

/// Fail if any recorded widget has zero width or zero height.
pub fn assert_all_nonzero(snap: &BoundsSnapshot, fixture_label: &str) {
    assert_all_nonzero_except(snap, fixture_label, &[])
}

/// Like `assert_all_nonzero`, but lets callers skip widget types whose
/// zero-size bounds are legitimate in this fixture.
pub fn assert_all_nonzero_except(snap: &BoundsSnapshot, fixture_label: &str, allow_zero: &[&str]) {
    let zeros: Vec<_> = snap
        .entries
        .iter()
        .filter(|(_, info)| info.width <= 0.0 || info.height <= 0.0)
        .filter(|(_, info)| !allow_zero.contains(&info.widget_type.as_str()))
        .collect();

    if !zeros.is_empty() {
        let mut msg = format!(
            "layout invariant violated (nonzero): {} widget(s) have zero width or height in fixture `{fixture_label}`\n",
            zeros.len()
        );
        for (id, info) in &zeros {
            msg.push_str(&format!(
                "  {id:24} {:>6.1}×{:<6.1}\n",
                info.width, info.height
            ));
        }
        msg.push_str("\nfull snapshot:\n");
        msg.push_str(&snap.dump());
        panic!("{msg}");
    }
}

/// Fail if any tracked widget's bounds extend outside its immediate tracked
/// parent's bounds.
pub fn assert_containment(snap: &BoundsSnapshot, fixture_label: &str, allow_overflow: &[&str]) {
    let by_id: std::collections::HashMap<&str, &ElementInfo> = snap
        .entries
        .iter()
        .map(|(id, info)| (id.as_str(), info))
        .collect();

    // Ancestor types that visually clip their descendants via `overflow_hidden`
    // (or equivalent). Taffy reports descendant bounds without regard to this
    // clipping, so containment would spuriously flag any closed shrink drawer
    // or off-screen scrolled row. Walk ancestors: if any is one of these
    // clipping types, exempt the descendant from containment.
    const CLIPPING_PARENT_TYPES: &[&str] = &["reactive_shell", "drawer"];
    let mut inside_scrollable: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (id, info) in &snap.entries {
        let mut cursor_parent = info.parent_id.as_deref();
        while let Some(pid) = cursor_parent {
            if let Some(parent) = by_id.get(pid) {
                if CLIPPING_PARENT_TYPES.contains(&parent.widget_type.as_str()) {
                    inside_scrollable.insert(id.as_str());
                    break;
                }
                cursor_parent = parent.parent_id.as_deref();
            } else {
                break;
            }
        }
    }

    let mut violations: Vec<String> = Vec::new();

    for (id, info) in &snap.entries {
        if allow_overflow.contains(&info.widget_type.as_str()) {
            continue;
        }
        if inside_scrollable.contains(id.as_str()) {
            continue;
        }
        let Some(parent_id) = info.parent_id.as_ref() else {
            continue;
        };
        let Some(parent) = by_id.get(parent_id.as_str()) else {
            continue;
        };
        let child_rect = Rect::of(info);
        let parent_rect = Rect::of(parent);
        if !child_rect.inside(parent_rect) {
            violations.push(format!(
                "  {id:24} {:>6.1}×{:<6.1} @ ({:>6.1},{:>6.1}) escapes parent {parent_id} {:>6.1}×{:<6.1} @ ({:>6.1},{:>6.1})",
                info.width, info.height, info.x, info.y,
                parent.width, parent.height, parent.x, parent.y,
            ));
        }
    }

    if !violations.is_empty() {
        let mut msg = format!(
            "layout invariant violated (containment): {} widget(s) escape their parent in fixture `{fixture_label}`\n",
            violations.len()
        );
        for v in &violations {
            msg.push_str(v);
            msg.push('\n');
        }
        msg.push_str("\nfull snapshot:\n");
        msg.push_str(&snap.dump());
        panic!("{msg}");
    }
}

/// Fail if any two widgets that share an immediate tracked parent have
/// non-trivial overlap.
pub fn assert_no_sibling_overlap(
    snap: &BoundsSnapshot,
    fixture_label: &str,
    allow_overlap_parents: &[&str],
) {
    let by_id: std::collections::HashMap<&str, &ElementInfo> = snap
        .entries
        .iter()
        .map(|(id, info)| (id.as_str(), info))
        .collect();

    let mut groups: std::collections::HashMap<&str, Vec<(&str, &ElementInfo)>> =
        std::collections::HashMap::new();
    for (id, info) in &snap.entries {
        if let Some(pid) = info.parent_id.as_ref() {
            groups.entry(pid.as_str()).or_default().push((id, info));
        }
    }

    let mut violations: Vec<String> = Vec::new();

    for (parent_id, siblings) in groups {
        if let Some(parent_info) = by_id.get(parent_id) {
            if allow_overlap_parents.contains(&parent_info.widget_type.as_str()) {
                continue;
            }
        }
        let visible: Vec<_> = siblings
            .iter()
            .filter(|(_, info)| info.width > 0.0 && info.height > 0.0)
            .collect();
        for i in 0..visible.len() {
            for j in (i + 1)..visible.len() {
                let (a_id, a_info) = visible[i];
                let (b_id, b_info) = visible[j];
                if Rect::of(a_info).overlaps(Rect::of(b_info)) {
                    violations.push(format!(
                        "  siblings of {parent_id}: {a_id} ({:>6.1},{:>6.1} {:>6.1}×{:<6.1}) overlaps {b_id} ({:>6.1},{:>6.1} {:>6.1}×{:<6.1})",
                        a_info.x, a_info.y, a_info.width, a_info.height,
                        b_info.x, b_info.y, b_info.width, b_info.height,
                    ));
                }
            }
        }
    }

    if !violations.is_empty() {
        let mut msg = format!(
            "layout invariant violated (sibling overlap): {} pair(s) overlap in fixture `{fixture_label}`\n",
            violations.len()
        );
        for v in &violations {
            msg.push_str(v);
            msg.push('\n');
        }
        msg.push_str("\nfull snapshot:\n");
        msg.push_str(&snap.dump());
        panic!("{msg}");
    }
}

/// Fail if any `reactive_shell` in the snapshot has zero visible leaf
/// descendants despite having some descendants.
pub fn assert_content_fidelity(snap: &BoundsSnapshot, fixture_label: &str) {
    let mut children_of: std::collections::HashMap<&str, Vec<&(String, ElementInfo)>> =
        std::collections::HashMap::new();
    for entry in &snap.entries {
        if let Some(pid) = entry.1.parent_id.as_deref() {
            children_of.entry(pid).or_default().push(entry);
        }
    }

    let shells: Vec<&(String, ElementInfo)> = snap
        .entries
        .iter()
        .filter(|(_, info)| info.widget_type == "reactive_shell")
        .collect();

    let mut violations: Vec<String> = Vec::new();

    for (shell_id, shell_info) in &shells {
        let mut total_descendants = 0usize;
        let mut visible_leaves = 0usize;
        let mut stack: Vec<&str> = vec![shell_id.as_str()];
        while let Some(node_id) = stack.pop() {
            if let Some(kids) = children_of.get(node_id) {
                for (kid_id, kid_info) in kids {
                    total_descendants += 1;
                    if VISIBLE_LEAF_TYPES.contains(&kid_info.widget_type.as_str()) {
                        visible_leaves += 1;
                    }
                    stack.push(kid_id.as_str());
                }
            }
        }

        if total_descendants > 0 && visible_leaves == 0 {
            violations.push(format!(
                "  {shell_id:24} {:>6.1}×{:<6.1} @ ({:>6.1},{:>6.1}) has {total_descendants} descendant(s) but 0 visible leaf widgets",
                shell_info.width, shell_info.height, shell_info.x, shell_info.y,
            ));
        }
    }

    if !violations.is_empty() {
        let mut msg = format!(
            "layout invariant violated (content fidelity): {} reactive_shell(s) laid out \
             without visible content in fixture `{fixture_label}`\n",
            violations.len()
        );
        for v in &violations {
            msg.push_str(v);
            msg.push('\n');
        }
        msg.push_str("\nfull snapshot:\n");
        msg.push_str(&snap.dump());
        panic!("{msg}");
    }
}

/// Run all five layout invariants with default skip-lists.
pub fn assert_layout_ok(snap: &BoundsSnapshot, fixture_label: &str) {
    assert_nonempty(snap, fixture_label);
    // `drawer`: overlay drawers are always zero-width in flow layout;
    // shrink drawers are zero-width when closed. Both are intentional.
    // `live_block`: deferred live_blocks start with empty streaming collections
    // (zero intrinsic height) until their tokio driver delivers data.
    assert_all_nonzero_except(snap, fixture_label, &["drawer", "live_block"]);
    assert_containment(snap, fixture_label, &["drawer", "pie_menu"]);
    // `view_mode_switcher` wraps a full-size slot plus an absolute-positioned
    // switcher_bar — the switcher buttons intentionally overlay the slot's
    // visible content.
    assert_no_sibling_overlap(snap, fixture_label, &["view_mode_switcher"]);
    assert_content_fidelity(snap, fixture_label);
}
