//! Regular languages over chars: Thompson NFA, subset-construction DFA,
//! product constructions, and the two static checks string lenses need
//! (disjoint union, unambiguous concatenation) with witness strings.
//!
//! Every language of one declaration shares one [`Alphabet`]: the coarsest
//! partition of all chars into classes that no regex of the declaration tells
//! apart. A DFA then moves on class ids, so a class such as `\p{L}` with its
//! hundreds of char ranges costs one column instead of hundreds.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;

use regex_syntax::hir::Class;
use regex_syntax::hir::Hir;
use regex_syntax::hir::HirKind;

const DEAD: u32 = u32::MAX;
const LAST_CHAR: u32 = char::MAX as u32;

pub struct Alphabet {
    /// Sorted starts of the elementary intervals; `starts[0] == 0`.
    starts: Vec<u32>,
    class_of: Vec<u32>,
    /// Per class: its intervals (inclusive).
    members: Vec<Vec<(u32, u32)>>,
    /// Per class: the char witness strings and messages use.
    reps: Vec<char>,
    /// Class ids, readable representatives first.
    readable_order: Vec<u32>,
}

impl Alphabet {
    /// The partition that keeps every char set of `regexes` a union of
    /// classes. A literal char counts as a one-char set.
    pub fn of(regexes: &[&Hir]) -> Alphabet {
        let mut sets: Vec<Vec<(u32, u32)>> = Vec::new();
        for h in regexes {
            char_sets(h, &mut sets);
        }
        let mut cuts: Vec<u32> = vec![0];
        for &(lo, hi) in sets.iter().flatten() {
            cuts.push(lo);
            if hi < LAST_CHAR {
                cuts.push(hi + 1);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        let words = sets.len().div_ceil(64).max(1);
        let mut signature = vec![vec![0u64; words]; cuts.len()];
        for (j, set) in sets.iter().enumerate() {
            for &(lo, hi) in set {
                let a = cuts.binary_search(&lo).expect("every range start is a cut");
                let b = if hi < LAST_CHAR {
                    cuts.binary_search(&(hi + 1))
                        .expect("every range end is a cut")
                } else {
                    cuts.len()
                };
                for sig in &mut signature[a..b] {
                    sig[j / 64] |= 1 << (j % 64);
                }
            }
        }
        let mut ids: HashMap<&[u64], u32> = HashMap::new();
        let mut class_of = Vec::with_capacity(cuts.len());
        let mut members: Vec<Vec<(u32, u32)>> = Vec::new();
        for (i, sig) in signature.iter().enumerate() {
            let next = members.len() as u32;
            let id = *ids.entry(sig.as_slice()).or_insert(next);
            if id == next {
                members.push(Vec::new());
            }
            let hi = cuts.get(i + 1).map_or(LAST_CHAR, |c| c - 1);
            members[id as usize].push((cuts[i], hi));
            class_of.push(id);
        }
        let reps: Vec<char> = members.iter().map(|m| representative(m)).collect();
        let mut readable_order: Vec<u32> = (0..members.len() as u32).collect();
        readable_order.sort_by_key(|&c| !is_readable(reps[c as usize]));
        Alphabet {
            starts: cuts,
            class_of,
            members,
            reps,
            readable_order,
        }
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    fn class(&self, c: char) -> u32 {
        let i = self.starts.partition_point(|&s| s <= c as u32) - 1;
        self.class_of[i]
    }

    /// The classes making up `ranges`, which must be one of the sets the
    /// alphabet was built from.
    fn classes_of(&self, ranges: impl Iterator<Item = (u32, u32)>) -> BTreeSet<u32> {
        let mut out = BTreeSet::new();
        for (lo, hi) in ranges {
            let a = self
                .starts
                .binary_search(&lo)
                .expect("alphabet built from this class");
            let b = self.starts.partition_point(|&s| s <= hi);
            out.extend(&self.class_of[a..b]);
        }
        out
    }

    fn describe(&self, class: u32) -> String {
        let m = &self.members[class as usize];
        let size: u64 = m.iter().map(|&(lo, hi)| u64::from(hi - lo + 1)).sum();
        if m.len() > 3 {
            return format!(
                "one of {size} characters such as {:?}",
                self.reps[class as usize]
            );
        }
        let shown: Vec<String> = m
            .iter()
            .map(|&(lo, hi)| match (char::from_u32(lo), char::from_u32(hi)) {
                (Some(lo), Some(hi)) if lo == hi => format!("{lo:?}"),
                (Some(lo), Some(hi)) => format!("{lo:?}..{hi:?}"),
                _ => format!("U+{lo:04X}..U+{hi:04X}"),
            })
            .collect();
        shown.join(" or ")
    }

    fn size(&self, class: u32) -> u64 {
        self.members[class as usize]
            .iter()
            .map(|&(lo, hi)| u64::from(hi - lo + 1))
            .sum()
    }
}

fn char_sets(h: &Hir, out: &mut Vec<Vec<(u32, u32)>>) {
    match h.kind() {
        HirKind::Literal(lit) => {
            for c in String::from_utf8_lossy(&lit.0).chars() {
                out.push(vec![(c as u32, c as u32)]);
            }
        }
        HirKind::Class(Class::Unicode(cls)) => out.push(
            cls.ranges()
                .iter()
                .map(|r| (r.start() as u32, r.end() as u32))
                .collect(),
        ),
        HirKind::Class(Class::Bytes(_)) | HirKind::Empty | HirKind::Look(_) => {}
        HirKind::Capture(c) => char_sets(&c.sub, out),
        HirKind::Repetition(r) => char_sets(&r.sub, out),
        HirKind::Concat(hs) | HirKind::Alternation(hs) => {
            for h in hs {
                char_sets(h, out);
            }
        }
    }
}

fn is_readable(c: char) -> bool {
    c.is_alphanumeric() || c == ' ' || c.is_ascii_punctuation()
}

fn representative(intervals: &[(u32, u32)]) -> char {
    let inside = |c: char| {
        intervals
            .iter()
            .any(|&(lo, hi)| lo <= c as u32 && c as u32 <= hi)
    };
    ['a', '0', ' ', 'A', '!']
        .into_iter()
        .chain((0x21u8..0x7f).map(char::from))
        .find(|&c| inside(c))
        .or_else(|| {
            intervals
                .iter()
                .flat_map(|&(lo, hi)| (lo..=hi).take(0x900))
                .find_map(char::from_u32)
        })
        .expect("a class holds at least one char")
}

#[derive(Default)]
struct Nfa {
    eps: Vec<Vec<usize>>,
    trans: Vec<Vec<(u32, usize)>>,
}

impl Nfa {
    fn state(&mut self) -> usize {
        self.eps.push(vec![]);
        self.trans.push(vec![]);
        self.eps.len() - 1
    }

    fn hir(&mut self, h: &Hir, alpha: &Alphabet) -> Result<(usize, usize), String> {
        let (s, e) = (self.state(), self.state());
        match h.kind() {
            HirKind::Empty => self.eps[s].push(e),
            HirKind::Literal(lit) => {
                let text = std::str::from_utf8(&lit.0).map_err(|e| e.to_string())?;
                let mut cur = s;
                for c in text.chars() {
                    let n = self.state();
                    self.trans[cur].push((alpha.class(c), n));
                    cur = n;
                }
                self.eps[cur].push(e);
            }
            HirKind::Class(Class::Unicode(cls)) => {
                let ranges = cls
                    .ranges()
                    .iter()
                    .map(|r| (r.start() as u32, r.end() as u32));
                for c in alpha.classes_of(ranges) {
                    self.trans[s].push((c, e));
                }
            }
            HirKind::Class(Class::Bytes(_)) => return Err("byte classes are not supported".into()),
            HirKind::Look(l) => {
                return Err(format!(
                    "anchors and look-around ({l:?}) are not supported: a format always matches \
                     the whole field"
                ));
            }
            HirKind::Capture(c) => {
                let (a, b) = self.hir(&c.sub, alpha)?;
                self.eps[s].push(a);
                self.eps[b].push(e);
            }
            HirKind::Concat(parts) => {
                let mut cur = s;
                for p in parts {
                    let (a, b) = self.hir(p, alpha)?;
                    self.eps[cur].push(a);
                    cur = b;
                }
                self.eps[cur].push(e);
            }
            HirKind::Alternation(alts) => {
                for p in alts {
                    let (a, b) = self.hir(p, alpha)?;
                    self.eps[s].push(a);
                    self.eps[b].push(e);
                }
            }
            HirKind::Repetition(rep) => {
                let mut cur = s;
                for _ in 0..rep.min {
                    let (a, b) = self.hir(&rep.sub, alpha)?;
                    self.eps[cur].push(a);
                    cur = b;
                }
                match rep.max {
                    None => {
                        let (a, b) = self.hir(&rep.sub, alpha)?;
                        self.eps[cur].push(a);
                        self.eps[b].push(cur);
                    }
                    Some(max) => {
                        for _ in rep.min..max {
                            let (a, b) = self.hir(&rep.sub, alpha)?;
                            self.eps[cur].push(a);
                            self.eps[cur].push(e);
                            cur = b;
                        }
                    }
                }
                self.eps[cur].push(e);
            }
        }
        Ok((s, e))
    }

    /// Copies `d` in; returns (start, single accept).
    fn embed(&mut self, d: &Dfa) -> (usize, usize) {
        let base = self.eps.len();
        for _ in 0..d.states() {
            self.state();
        }
        let end = self.state();
        for i in 0..d.states() {
            for c in 0..d.k as u32 {
                if let Some(t) = d.next(i, c) {
                    self.trans[base + i].push((c, base + t));
                }
            }
            if d.accept[i] {
                self.eps[base + i].push(end);
            }
        }
        (base, end)
    }

    fn closure(&self, set: &mut BTreeSet<usize>) {
        let mut stack: Vec<usize> = set.iter().copied().collect();
        while let Some(s) = stack.pop() {
            for &n in &self.eps[s] {
                if set.insert(n) {
                    stack.push(n);
                }
            }
        }
    }

    fn determinize(&self, alpha: &Arc<Alphabet>, start: usize, accept: usize) -> Dfa {
        let k = alpha.len();
        let mut first = BTreeSet::from([start]);
        self.closure(&mut first);
        let mut ids: HashMap<BTreeSet<usize>, usize> = HashMap::from([(first.clone(), 0)]);
        let mut sets = vec![first];
        let mut dfa = Dfa::empty(alpha);
        let mut i = 0;
        while i < sets.len() {
            let mut buckets: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); k];
            for &s in &sets[i] {
                for &(c, t) in &self.trans[s] {
                    buckets[c as usize].insert(t);
                }
            }
            dfa.accept.push(sets[i].contains(&accept));
            for mut target in buckets {
                if target.is_empty() {
                    dfa.trans.push(DEAD);
                    continue;
                }
                self.closure(&mut target);
                let next = sets.len();
                let id = *ids.entry(target.clone()).or_insert_with(|| {
                    sets.push(target);
                    next
                });
                dfa.trans.push(id as u32);
            }
            i += 1;
        }
        dfa
    }
}

/// A partial DFA over class ids: a missing transition goes to the implicit
/// dead state. State 0 is the start.
#[derive(Clone)]
pub struct Dfa {
    alpha: Arc<Alphabet>,
    k: usize,
    /// `states × k`, row-major; [`DEAD`] where there is no transition.
    trans: Vec<u32>,
    accept: Vec<bool>,
}

impl std::fmt::Debug for Dfa {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Dfa({} states over {} classes)", self.states(), self.k)
    }
}

impl Dfa {
    fn empty(alpha: &Arc<Alphabet>) -> Dfa {
        Dfa {
            alpha: alpha.clone(),
            k: alpha.len(),
            trans: vec![],
            accept: vec![],
        }
    }

    pub fn regex(alpha: &Arc<Alphabet>, hir: &Hir) -> Result<Dfa, String> {
        let mut n = Nfa::default();
        let (s, e) = n.hir(hir, alpha)?;
        Ok(n.determinize(alpha, s, e))
    }

    pub fn epsilon(alpha: &Arc<Alphabet>) -> Dfa {
        let mut d = Dfa::empty(alpha);
        d.trans = vec![DEAD; d.k];
        d.accept = vec![true];
        d
    }

    pub fn concat(alpha: &Arc<Alphabet>, parts: &[&Dfa]) -> Dfa {
        let mut n = Nfa::default();
        let start = n.state();
        let mut cur = start;
        for d in parts {
            let (a, b) = n.embed(d);
            n.eps[cur].push(a);
            cur = b;
        }
        n.determinize(alpha, start, cur)
    }

    pub fn union(alpha: &Arc<Alphabet>, parts: &[&Dfa]) -> Dfa {
        let mut n = Nfa::default();
        let (start, end) = (n.state(), n.state());
        for d in parts {
            let (a, b) = n.embed(d);
            n.eps[start].push(a);
            n.eps[b].push(end);
        }
        n.determinize(alpha, start, end)
    }

    pub fn star(&self) -> Dfa {
        let mut n = Nfa::default();
        let start = n.state();
        let (a, b) = n.embed(self);
        n.eps[start].push(a);
        n.eps[b].push(start);
        n.determinize(&self.alpha, start, start)
    }

    pub fn optional(&self) -> Dfa {
        Dfa::union(&self.alpha, &[self, &Dfa::epsilon(&self.alpha)])
    }

    pub fn intersect(&self, other: &Dfa) -> Dfa {
        product(
            self,
            other,
            |a, b| a == Some(true) && b == Some(true),
            false,
        )
    }

    pub fn minus(&self, other: &Dfa) -> Dfa {
        product(self, other, |a, b| a == Some(true) && b != Some(true), true)
    }

    fn next(&self, state: usize, class: u32) -> Option<usize> {
        let t = self.trans[state * self.k + class as usize];
        (t != DEAD).then_some(t as usize)
    }

    pub fn step(&self, state: usize, c: char) -> Option<usize> {
        self.next(state, self.alpha.class(c))
    }

    pub fn is_accepting(&self, state: usize) -> bool {
        self.accept[state]
    }

    pub fn accepts(&self, s: &str) -> bool {
        let mut st = 0;
        for c in s.chars() {
            match self.step(st, c) {
                Some(n) => st = n,
                None => return false,
            }
        }
        self.accept[st]
    }

    pub fn accepts_empty(&self) -> bool {
        self.accept[0]
    }

    /// A shortest member, if the language is not empty.
    pub fn example(&self) -> Option<String> {
        shortest_to(self, 0, |s| self.accept[s])
    }

    /// Where `s` leaves the language: (char offset, what would have been
    /// accepted there).
    pub fn mismatch(&self, s: &str) -> (usize, String) {
        let mut st = 0;
        for (i, c) in s.chars().enumerate() {
            match self.step(st, c) {
                Some(n) => st = n,
                None => return (i, self.expected(st)),
            }
        }
        (s.chars().count(), self.expected(st))
    }

    fn expected(&self, st: usize) -> String {
        let alpha = &self.alpha;
        let (allowed, refused): (Vec<u32>, Vec<u32>) =
            (0..self.k as u32).partition(|&c| self.next(st, c).is_some());
        let covered: u64 = allowed.iter().map(|&c| alpha.size(c)).sum();
        let mut parts: Vec<String> = if covered > 0x8000 {
            let shown: Vec<String> = refused.iter().take(6).map(|&c| alpha.describe(c)).collect();
            vec![format!("any character except {}", shown.join(", "))]
        } else {
            let mut shown: Vec<String> =
                allowed.iter().take(6).map(|&c| alpha.describe(c)).collect();
            if allowed.len() > 6 {
                shown.push(format!("… ({} more)", allowed.len() - 6));
            }
            shown
        };
        if self.accept[st] {
            parts.push("end of text".into());
        }
        if parts.is_empty() {
            "nothing (dead end)".into()
        } else {
            parts.join(" or ")
        }
    }

    pub fn states(&self) -> usize {
        self.accept.len()
    }
}

fn shortest_to(d: &Dfa, from: usize, goal: impl Fn(usize) -> bool) -> Option<String> {
    let mut prev: HashMap<usize, (usize, char)> = HashMap::new();
    let mut seen = BTreeSet::from([from]);
    let mut q = VecDeque::from([from]);
    while let Some(s) = q.pop_front() {
        if goal(s) {
            let mut out = vec![];
            let mut cur = s;
            while cur != from {
                let (p, c) = prev[&cur];
                out.push(c);
                cur = p;
            }
            return Some(out.into_iter().rev().collect());
        }
        for &c in &d.alpha.readable_order {
            if let Some(t) = d.next(s, c)
                && seen.insert(t)
            {
                prev.insert(t, (s, d.alpha.reps[c as usize]));
                q.push_back(t);
            }
        }
    }
    None
}

fn product(
    a: &Dfa,
    b: &Dfa,
    acc: impl Fn(Option<bool>, Option<bool>) -> bool,
    keep_b_dead: bool,
) -> Dfa {
    assert!(
        Arc::ptr_eq(&a.alpha, &b.alpha),
        "languages of one declaration share one alphabet"
    );
    let key0 = (0usize, Some(0usize));
    let mut ids = HashMap::from([(key0, 0usize)]);
    let mut keys = vec![key0];
    let mut dfa = Dfa::empty(&a.alpha);
    let mut i = 0;
    while i < keys.len() {
        let (sa, sb) = keys[i];
        dfa.accept
            .push(acc(Some(a.accept[sa]), sb.map(|s| b.accept[s])));
        for c in 0..a.k as u32 {
            let na = a.next(sa, c);
            let nb = sb.and_then(|s| b.next(s, c));
            let Some(na) = na.filter(|_| nb.is_some() || keep_b_dead) else {
                dfa.trans.push(DEAD);
                continue;
            };
            let next = keys.len();
            let id = *ids.entry((na, nb)).or_insert_with(|| {
                keys.push((na, nb));
                next
            });
            dfa.trans.push(id as u32);
        }
        i += 1;
    }
    dfa
}

/// A string with two different splits into L1·L2: (whole, first split point,
/// second split point) in bytes.
pub struct ConcatWitness {
    pub text: String,
    pub split_a: usize,
    pub split_b: usize,
}

/// Unambiguous concatenation (Boomerang's `L1 ·! L2`): no string splits two
/// ways. Ambiguous iff there is a non-empty w with u, uw ∈ L1 and wv, v ∈ L2.
pub fn concat_ambiguity(l1: &Dfa, l2: &Dfa) -> Option<ConcatWitness> {
    let alpha = &l1.alpha;
    // States q of L2 for which some v ∈ L2 also leads q to acceptance, with that v.
    let mut q2: HashMap<usize, String> = HashMap::new();
    for q in 0..l2.states() {
        if let Some(v) = shortest_both(l2, q, l2, 0) {
            q2.insert(q, v);
        }
    }
    let mut prev: HashMap<(usize, usize), ((usize, usize), char)> = HashMap::new();
    let mut queue = VecDeque::new();
    let mut seen = BTreeSet::new();
    for a in (0..l1.states()).filter(|&s| l1.accept[s]) {
        seen.insert((a, 0));
        queue.push_back((a, 0, 0usize));
    }
    while let Some((p1, p2, len)) = queue.pop_front() {
        if len > 0 && l1.accept[p1] && q2.contains_key(&p2) {
            let mut w = vec![];
            let mut cur = (p1, p2);
            while w.len() < len {
                let (p, c) = prev[&cur];
                w.push(c);
                cur = p;
            }
            let w: String = w.into_iter().rev().collect();
            let u = shortest_to(l1, 0, |s| s == cur.0).expect("a BFS source is reachable");
            let v = &q2[&p2];
            return Some(ConcatWitness {
                text: format!("{u}{w}{v}"),
                split_a: u.len(),
                split_b: u.len() + w.len(),
            });
        }
        for &c in &alpha.readable_order {
            if let (Some(n1), Some(n2)) = (l1.next(p1, c), l2.next(p2, c))
                && seen.insert((n1, n2))
            {
                prev.insert((n1, n2), ((p1, p2), alpha.reps[c as usize]));
                queue.push_back((n1, n2, len + 1));
            }
        }
    }
    None
}

/// A shortest v that leads `a` from `sa` and `b` from `sb` to acceptance.
fn shortest_both(a: &Dfa, sa: usize, b: &Dfa, sb: usize) -> Option<String> {
    let mut prev: HashMap<(usize, usize), ((usize, usize), char)> = HashMap::new();
    let mut seen = BTreeSet::from([(sa, sb)]);
    let mut q = VecDeque::from([(sa, sb)]);
    while let Some((x, y)) = q.pop_front() {
        if a.accept[x] && b.accept[y] {
            let mut out = vec![];
            let mut cur = (x, y);
            while cur != (sa, sb) {
                let (p, c) = prev[&cur];
                out.push(c);
                cur = p;
            }
            return Some(out.into_iter().rev().collect());
        }
        for &c in &a.alpha.readable_order {
            if let (Some(nx), Some(ny)) = (a.next(x, c), b.next(y, c))
                && seen.insert((nx, ny))
            {
                prev.insert((nx, ny), ((x, y), a.alpha.reps[c as usize]));
                q.push_back((nx, ny));
            }
        }
    }
    None
}
