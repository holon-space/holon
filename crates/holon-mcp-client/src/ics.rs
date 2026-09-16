//! An iCalendar (RFC 5545) `VEVENT` decoder: the `ics` sibling of the Atom and
//! RSS codecs, so a calendar URL from ANY provider becomes a replica with no
//! transport, no consent screen and no OAuth client.
//!
//! Scope is what a read replica needs and nothing more. A `VEVENT` becomes one
//! record per occurrence, keyed `uid` when it does not recur and
//! `uid@<instant>` when it does, so an expanded occurrence and a
//! `RECURRENCE-ID` override that names it land on the SAME key and the override
//! takes its occurrence's place under replace-scope semantics.
//!
//! `VTIMEZONE` blocks are accepted, and a `TZID` is resolved through the tz
//! database rather than by replaying the block's offsets: a zone NAME is a
//! fact, a pair of offsets is a reimplementation of one.
//!
//! Fail loud, never skip. Under replace-scope semantics a skipped record reads
//! as a deletion, so a component or value this decoder does not model
//! (`VTODO`, `VJOURNAL`, `VFREEBUSY`, `DURATION`, an unknown `VALUE` type, an
//! unresolvable `TZID`) fails the WHOLE feed by name. A `VALARM` and any other
//! component nested inside a `VEVENT` carries no row and is ignored, which is
//! not a skip: nothing was going to be recorded for it either way.

use anyhow::Result;
use anyhow::bail;
use chrono::DateTime;
use chrono::NaiveDate;
use chrono::NaiveDateTime;
use chrono::SecondsFormat;
use chrono::TimeZone;
use chrono::Utc;
use chrono_tz::Tz as ChronoTz;
use rrule::RRule;
use rrule::RRuleSet;
use rrule::Tz as RruleTz;
use rrule::Unvalidated;
use serde::Deserialize;
use serde::Serialize;

use crate::mcp_sidecar::SyncInterval;

/// How far either side of "now" a recurrence is expanded.
///
/// A published feed is the whole history, so expansion is always bounded.
/// Occurrences outside the window are NOT replicated, and a sidecar says so:
/// an unbounded expansion of a decade of a daily standup is not a feature.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IcsWindow {
    #[serde(default = "IcsWindow::default_back")]
    pub back: SyncInterval,
    #[serde(default = "IcsWindow::default_forward")]
    pub forward: SyncInterval,
}

impl IcsWindow {
    fn default_back() -> SyncInterval {
        SyncInterval::parse("30d").expect("a literal default parses")
    }

    fn default_forward() -> SyncInterval {
        SyncInterval::parse("365d").expect("a literal default parses")
    }
}

impl Default for IcsWindow {
    fn default() -> Self {
        Self {
            back: Self::default_back(),
            forward: Self::default_forward(),
        }
    }
}

/// Occurrences one event may expand to inside the window. Reaching it FAILS
/// LOUD rather than truncating: a silently short expansion reads as a deletion
/// of every occurrence past the bound.
const MAX_OCCURRENCES: u16 = 10_000;

/// One property: name, parameters and raw value, with the physical line it
/// started on so a refusal can name it.
struct Prop {
    name: String,
    params: Vec<(String, String)>,
    value: String,
    line: usize,
}

impl Prop {
    fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A `VEVENT`, in file order.
struct Vevent {
    props: Vec<Prop>,
    line: usize,
}

impl Vevent {
    fn get(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.name == name)
    }

    fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Prop> + 'a {
        self.props.iter().filter(move |p| p.name == name)
    }

    fn text(&self, name: &str) -> Option<String> {
        self.get(name).map(|p| unescape_text(&p.value))
    }
}

/// A DATE or DATE-TIME that has been resolved to a real instant.
struct Moment {
    all_day: bool,
    /// The line the value was written on, and the value as written, so a
    /// refusal can name what the feed actually said.
    line: usize,
    raw: String,
    /// The calendar DATE. For an all-day value this is the date AS WRITTEN,
    /// which is NOT the UTC date of its midnight: midnight in Europe/Berlin is
    /// 22:00Z the day before, so deriving the date from the instant would move
    /// every all-day event back a day.
    date: NaiveDate,
    /// The instant, UTC.
    utc: DateTime<Utc>,
    /// The value's own zone and wall-clock time, which recurrence anchors on
    /// (`all` expands "in the same time zone as the dt_start property").
    zoned: DateTime<RruleTz>,
    tz_name: String,
}

/// One decoded event, ready to emit.
struct Event {
    uid: String,
    start: Moment,
    end: Option<Moment>,
    recurrence_id: Option<Moment>,
    rrule: Option<String>,
    exdates: Vec<Moment>,
    summary: Option<String>,
    description: Option<String>,
    location: Option<String>,
    status: Option<String>,
    updated: Option<String>,
    line: usize,
}

/// Decode an iCalendar document into one record per occurrence.
pub fn decode(body: &str, window: IcsWindow, now: DateTime<Utc>) -> Result<Vec<serde_json::Value>> {
    let (calendar_tz, vevents) = parse_calendar(body)?;
    let window_start = now - chrono::Duration::seconds(window.back.duration().as_secs() as i64);
    let window_end = now + chrono::Duration::seconds(window.forward.duration().as_secs() as i64);

    let mut events: Vec<Event> = Vec::with_capacity(vevents.len());
    for vevent in &vevents {
        events.push(parse_event(vevent, calendar_tz)?);
    }

    // Overrides are keyed by the instant they replace, so a generated
    // occurrence and its override meet on one key.
    let mut override_keys: Vec<(String, DateTime<Utc>)> = Vec::new();
    for ev in &events {
        if let Some(rid) = &ev.recurrence_id {
            override_keys.push((ev.uid.clone(), rid.utc));
        }
    }

    let mut out: Vec<serde_json::Value> = Vec::new();
    for ev in &events {
        match (&ev.rrule, &ev.recurrence_id) {
            // An override: one row, keyed by the instant it replaces, carrying
            // its OWN times (which is how a moved instance is expressed).
            (_, Some(rid)) => {
                // Judged by the instant it REPLACES: that is the occurrence's
                // place in the series, whatever the override moved it to.
                if !in_window(rid, window_start, window_end) {
                    continue;
                }
                out.push(ev.row(&occurrence_id(&ev.uid, rid.utc), &ev.start, ev.end.as_ref()));
            }
            // A non-recurring master.
            (None, None) => {
                if !in_window(&ev.start, window_start, window_end) {
                    continue;
                }
                out.push(ev.row(&ev.uid, &ev.start, ev.end.as_ref()));
            }
            // A recurring master: expand, skipping instants an override owns.
            (Some(rrule), None) => {
                // An occurrence keeps its event's LENGTH, so the end moves by
                // the same offset the start does.
                let length = ev.end.as_ref().map(|end| end.utc - ev.start.utc);
                for instant in expand(ev, rrule, window_start, window_end)? {
                    if override_keys
                        .iter()
                        .any(|(uid, at)| uid == &ev.uid && *at == instant)
                    {
                        continue;
                    }
                    let start = shift(&ev.start, instant);
                    if !in_window(&start, window_start, window_end) {
                        continue;
                    }
                    let end = match (&ev.end, length) {
                        (Some(end), Some(length)) => Some(shift(end, instant + length)),
                        _ => None,
                    };
                    out.push(ev.row(&occurrence_id(&ev.uid, instant), &start, end.as_ref()));
                }
            }
        }
    }
    Ok(out)
}

/// The row key of one occurrence: `uid@<RFC3339 instant>`.
fn occurrence_id(uid: &str, instant: DateTime<Utc>) -> String {
    format!("{uid}@{}", rfc3339(instant))
}

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Move a master's start onto one occurrence instant, preserving the instant's
/// wall-clock reading in the event's own zone.
fn shift(master: &Moment, instant: DateTime<Utc>) -> Moment {
    let zoned = instant.with_timezone(&master.zoned.timezone());
    Moment {
        all_day: master.all_day,
        line: master.line,
        raw: master.raw.clone(),
        date: zoned.date_naive(),
        utc: instant,
        zoned,
        tz_name: master.tz_name.clone(),
    }
}

impl Event {
    /// One record. Keys are present only when the feed carried them, so a
    /// consumer can tell "absent" from "empty".
    fn row(&self, id: &str, start: &Moment, end: Option<&Moment>) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        let mut put = |k: &str, v: serde_json::Value| {
            obj.insert(k.to_string(), v);
        };
        put("id", serde_json::Value::String(id.to_string()));
        put("uid", serde_json::Value::String(self.uid.clone()));
        if let Some(rid) = &self.recurrence_id {
            put("recurrence_id", serde_json::Value::String(rfc3339(rid.utc)));
        }
        put("start", serde_json::Value::String(format_moment(start)));
        if let Some(end) = end {
            put("end", serde_json::Value::String(format_moment(end)));
        }
        // A whole number, because the schema column is INTEGER — a JSON bool
        // would have to be coerced somewhere less honest than here.
        put(
            "all_day",
            serde_json::Value::Number(if start.all_day { 1 } else { 0 }.into()),
        );
        put("tzid", serde_json::Value::String(start.tz_name.clone()));
        for (key, value) in [
            ("summary", &self.summary),
            ("description", &self.description),
            ("location", &self.location),
            ("status", &self.status),
            ("updated", &self.updated),
        ] {
            if let Some(v) = value {
                put(key, serde_json::Value::String(v.clone()));
            }
        }
        serde_json::Value::Object(obj)
    }
}

/// A DATE value is its own date; a DATE-TIME is its instant.
fn format_moment(m: &Moment) -> String {
    if m.all_day {
        m.date.format("%Y-%m-%d").to_string()
    } else {
        rfc3339(m.utc)
    }
}

/// Expand one event's `RRULE` inside the window.
fn expand(
    ev: &Event,
    rrule: &str,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
) -> Result<Vec<DateTime<Utc>>> {
    let unvalidated: RRule<Unvalidated> = rrule.parse().map_err(|e| {
        anyhow::anyhow!(
            "iCalendar line {}: RRULE '{rrule}' on '{}' does not parse: {e}",
            ev.line,
            ev.uid
        )
    })?;
    let validated = unvalidated.validate(ev.start.zoned).map_err(|e| {
        anyhow::anyhow!(
            "iCalendar line {}: RRULE '{rrule}' on '{}' is not satisfiable from its DTSTART: {e}",
            ev.line,
            ev.uid
        )
    })?;
    let set = RRuleSet::new(ev.start.zoned).rrule(validated);
    let tz = ev.start.zoned.timezone();
    // The pre-filter is deliberately WIDER than the window, by two days. The
    // window itself is applied ONCE, to every row, by `in_window`, which judges
    // an all-day row by its DATE; but an all-day occurrence's midnight INSTANT
    // is up to a day before the window instant (midnight in Berlin is 22:00Z the
    // previous day), so a filter asking rrule for the window's instants would
    // drop a row that `in_window` would keep. Widening here is what lets
    // `in_window` be the single authority instead of one of two disagreeing
    // conventions.
    let slack = chrono::Duration::days(2);
    let result = set
        .after((window_start - slack).with_timezone(&tz))
        .before((window_end + slack).with_timezone(&tz))
        .all(MAX_OCCURRENCES);
    if result.limited || result.dates.len() >= MAX_OCCURRENCES as usize {
        bail!(
            "iCalendar: event '{}' expands to at least {MAX_OCCURRENCES} occurrences in the \
             configured window, which is the bound. Narrow `ics_window` in the sidecar, or raise \
             MAX_OCCURRENCES; truncating here would read as a deletion of every occurrence past \
             the bound.",
            ev.uid
        );
    }
    let generated: Vec<DateTime<Utc>> = result
        .dates
        .into_iter()
        .map(|d| d.with_timezone(&Utc))
        .collect();

    // Exclusion is applied HERE rather than through `RRuleSet::exdate`, for two
    // reasons: an all-day EXDATE must match an occurrence's LOCAL DATE rather
    // than its midnight instant, and an EXDATE that excludes nothing must be
    // caught instead of passing in silence.
    let mut used = vec![false; ev.exdates.len()];
    let mut kept: Vec<DateTime<Utc>> = Vec::with_capacity(generated.len());
    for instant in generated {
        let matching: Vec<usize> = (0..ev.exdates.len())
            .filter(|i| exdate_matches(ev, *i, instant))
            .collect();
        if matching.is_empty() {
            kept.push(instant);
        } else {
            for i in matching {
                used[i] = true;
            }
        }
    }
    for (i, ex) in ev.exdates.iter().enumerate() {
        // Only an EXDATE inside the window is judged: one that names an
        // occurrence outside it has nothing here to match, which says nothing
        // about the feed. "Inside" is the SAME rule `in_window` applies to the
        // rows it targets, so an all-day EXDATE on the window's first day is
        // judged by its DATE — judged by its instant it would look like the
        // previous day and pass in silence.
        let judged = in_window(ex, window_start, window_end);
        if !used[i] && judged {
            bail!(
                "iCalendar line {}: the EXDATE '{}' on '{}' names no occurrence this series \
                 generates, so it excludes nothing. A written exclusion that removes nothing is \
                 a feed claiming an instance it will not deliver, and under replace-scope the \
                 silent reading is a missing row.",
                ex.line,
                ex.raw,
                ev.uid
            );
        }
    }
    Ok(kept)
}

/// Does EXDATE `i` name this occurrence?
///
/// An all-day EXDATE matches on the occurrence's LOCAL DATE: RFC 5545 requires
/// an EXDATE's value type to match DTSTART's, so a DATE EXDATE belongs to a
/// DATE series, and comparing instants across zones would miss it whenever the
/// two offsets differ. Everything else matches on the instant.
fn exdate_matches(ev: &Event, i: usize, instant: DateTime<Utc>) -> bool {
    let ex = &ev.exdates[i];
    if ex.all_day {
        let tz = ev.start.zoned.timezone();
        instant.with_timezone(&tz).date_naive() == ex.date
    } else {
        instant == ex.utc
    }
}

/// Is this row inside the replica's window?
///
/// The window is the BOUND for EVERY row, not only for an expansion: a one-off
/// event years back is not replicated either, and a row that ages out of `back`
/// on a later sync is dropped. That is what the sidecar documentation promises,
/// and it is why a one-off is judged here rather than only inside `expand`.
fn in_window(m: &Moment, window_start: DateTime<Utc>, window_end: DateTime<Utc>) -> bool {
    if m.all_day {
        // An all-day row IS a date, so it is judged on dates: its midnight
        // instant can sit on the far side of a boundary its date is inside.
        let tz = m.zoned.timezone();
        let from = window_start.with_timezone(&tz).date_naive();
        let to = window_end.with_timezone(&tz).date_naive();
        m.date >= from && m.date <= to
    } else {
        m.utc >= window_start && m.utc <= window_end
    }
}

/// The calendar's `X-WR-TIMEZONE` and its `VEVENT`s, in file order.
fn parse_calendar(body: &str) -> Result<(Option<ChronoTz>, Vec<Vevent>)> {
    let mut stack: Vec<(String, usize)> = Vec::new();
    let mut calendar_tz: Option<ChronoTz> = None;
    let mut vevents: Vec<Vevent> = Vec::new();
    let mut current: Option<Vevent> = None;
    let mut saw_calendar = false;

    for (line, raw) in unfold(body) {
        if raw.trim().is_empty() {
            continue;
        }
        let prop = parse_prop(&raw, line)?;
        match prop.name.as_str() {
            "BEGIN" => {
                let name = prop.value.trim().to_ascii_uppercase();
                if name == "VCALENDAR" {
                    saw_calendar = true;
                }
                if name == "VEVENT" {
                    current = Some(Vevent {
                        props: Vec::new(),
                        line,
                    });
                }
                stack.push((name, line));
            }
            "END" => {
                let name = prop.value.trim().to_ascii_uppercase();
                let (opened, opened_at) = stack.pop().ok_or_else(|| {
                    anyhow::anyhow!("iCalendar line {line}: END:{name} with no matching BEGIN")
                })?;
                if opened != name {
                    bail!(
                        "iCalendar line {line}: END:{name} closes BEGIN:{opened}, opened at line \
                         {opened_at}"
                    );
                }
                if name == "VEVENT" {
                    let vevent = current.take().ok_or_else(|| {
                        anyhow::anyhow!("iCalendar line {line}: END:VEVENT with no open VEVENT")
                    })?;
                    vevents.push(vevent);
                } else if matches!(
                    stack.last().map(|(parent, _)| parent.as_str()),
                    None | Some("VCALENDAR")
                ) {
                    refuse_unmodelled_component(&name, opened_at)?;
                }
            }
            _ => {
                let innermost = stack.last().map(|(name, _)| name.as_str()).ok_or_else(|| {
                    anyhow::anyhow!(
                        "iCalendar line {line}: property '{}' appears outside any component",
                        prop.name
                    )
                })?;
                if innermost == "VEVENT" {
                    if let Some(vevent) = current.as_mut() {
                        vevent.props.push(prop);
                    }
                } else if innermost == "VCALENDAR" && prop.name == "X-WR-TIMEZONE" {
                    // The one calendar-level property that is load bearing. The
                    // rest (PRODID, CALSCALE, X-WR-CALNAME) is presentation.
                    let tzid = prop.value.trim();
                    calendar_tz = Some(tzid.parse::<ChronoTz>().map_err(|_| {
                        anyhow::anyhow!(
                            "iCalendar line {line}: X-WR-TIMEZONE '{tzid}' is not an IANA \
                             timezone name this build can resolve, so a floating local time in \
                             this calendar has no decidable instant"
                        )
                    })?);
                }
                // Anything else is nested inside a VEVENT (VALARM) or inside a
                // VTIMEZONE (DAYLIGHT/STANDARD), or is another calendar-level
                // property: none of them carries a row.
            }
        }
    }

    if let Some((name, at)) = stack.first() {
        bail!("iCalendar: BEGIN:{name} at line {at} is never closed");
    }
    if !saw_calendar {
        bail!("iCalendar: no BEGIN:VCALENDAR — this document is not iCalendar");
    }
    Ok((calendar_tz, vevents))
}

/// A top-level component this decoder does not model. Refused by name: it holds
/// rows this replica would otherwise silently not have.
fn refuse_unmodelled_component(name: &str, line: usize) -> Result<()> {
    match name {
        // The document root, and the zone definitions. Neither is a row.
        "VCALENDAR" | "VTIMEZONE" => Ok(()),
        other => bail!(
            "iCalendar line {line}: component '{other}' is not modelled by the `ics` codec \
             (VTIMEZONE and VEVENT are). Skipping it would report an absence where the feed \
             carried a record."
        ),
    }
}

/// Split logical lines, rejoining a folded continuation (a line beginning with
/// a space or a tab) onto its predecessor. Each entry carries the physical line
/// the logical line STARTED on, so an error names a line a reader can open.
fn unfold(body: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    for (idx, raw) in body.lines().enumerate() {
        let line = idx + 1;
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        match raw.strip_prefix(' ').or_else(|| raw.strip_prefix('\t')) {
            Some(rest) => match out.last_mut() {
                Some((_, acc)) => acc.push_str(rest),
                None => out.push((line, rest.to_string())),
            },
            None => out.push((line, raw.to_string())),
        }
    }
    out
}

/// `NAME;PARAM=VALUE:VALUE`, where the value may itself contain `:` and a
/// parameter value may be quoted.
fn parse_prop(raw: &str, line: usize) -> Result<Prop> {
    let mut quoted = false;
    let mut colon = None;
    for (i, ch) in raw.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            ':' if !quoted => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon.ok_or_else(|| {
        anyhow::anyhow!("iCalendar line {line}: no ':' separating name from value: '{raw}'")
    })?;
    let (head, value) = raw.split_at(colon);
    let mut parts = head.split(';');
    let name = parts.next().unwrap_or("").trim().to_ascii_uppercase();
    if name.is_empty() {
        bail!("iCalendar line {line}: property has an empty name: '{raw}'");
    }
    let mut params = Vec::new();
    for part in parts {
        let (key, val) = part.split_once('=').ok_or_else(|| {
            anyhow::anyhow!("iCalendar line {line}: parameter '{part}' in '{name}' has no '='")
        })?;
        params.push((
            key.trim().to_ascii_uppercase(),
            val.trim().trim_matches('"').to_string(),
        ));
    }
    Ok(Prop {
        name,
        params,
        value: value[1..].to_string(),
        line,
    })
}

/// RFC 5545 TEXT escaping, reversed.
fn unescape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') | Some('N') => out.push('\n'),
            Some('\\') => out.push('\\'),
            Some(';') => out.push(';'),
            Some(',') => out.push(','),
            // An escape this build does not know is kept verbatim rather than
            // dropped: the value is what the feed said.
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// One `VEVENT` into a [`Event`], failing loud on anything it needs and does
/// not find.
fn parse_event(vevent: &Vevent, calendar_tz: Option<ChronoTz>) -> Result<Event> {
    let line = vevent.line;
    let uid = vevent
        .get("UID")
        .map(|p| unescape_text(&p.value).trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("iCalendar line {line}: this VEVENT has no usable UID, which is what identifies its row")
        })?;

    if vevent.get("DURATION").is_some() && vevent.get("DTEND").is_none() {
        bail!(
            "iCalendar line {line}: VEVENT '{uid}' expresses its length as DURATION, which this \
             codec does not model. Refusing rather than guessing an end: an invented end is \
             indistinguishable from a wrong one."
        );
    }

    let dtstart = vevent.get("DTSTART").ok_or_else(|| {
        anyhow::anyhow!(
            "iCalendar line {line}: VEVENT '{uid}' has no DTSTART, so it cannot be placed in time"
        )
    })?;
    let start = parse_moment(dtstart, calendar_tz, "DTSTART")?;

    let end = match vevent.get("DTEND") {
        Some(prop) => Some(parse_moment(prop, calendar_tz, "DTEND")?),
        None => None,
    };

    let recurrence_id = match vevent.get("RECURRENCE-ID") {
        Some(prop) => Some(parse_moment(prop, calendar_tz, "RECURRENCE-ID")?),
        None => None,
    };

    let mut rrules = vevent.all("RRULE");
    let rrule = rrules.next().map(|p| p.value.trim().to_string());
    if rrules.next().is_some() {
        bail!(
            "iCalendar line {line}: VEVENT '{uid}' declares more than one RRULE, which RFC 5545 does not allow"
        );
    }

    let mut exdates = Vec::new();
    for prop in vevent.all("EXDATE") {
        // EXDATE may carry a comma-separated list in one property.
        for one in prop.value.split(',') {
            let single = Prop {
                name: prop.name.clone(),
                params: prop.params.clone(),
                value: one.to_string(),
                line: prop.line,
            };
            if single.value.trim().is_empty() {
                continue;
            }
            exdates.push(parse_moment(&single, calendar_tz, "EXDATE")?);
        }
    }

    Ok(Event {
        uid,
        start,
        end,
        recurrence_id,
        rrule,
        exdates,
        summary: vevent.text("SUMMARY"),
        description: vevent.text("DESCRIPTION"),
        location: vevent.text("LOCATION"),
        status: vevent.text("STATUS"),
        updated: vevent
            .text("LAST-MODIFIED")
            .or_else(|| vevent.text("DTSTAMP")),
        line,
    })
}

/// A `DATE` or `DATE-TIME` value into a resolved instant.
fn parse_moment(prop: &Prop, calendar_tz: Option<ChronoTz>, what: &str) -> Result<Moment> {
    let line = prop.line;
    let raw = prop.value.trim();
    let date_value = prop
        .param("VALUE")
        .is_some_and(|v| v.eq_ignore_ascii_case("DATE"));

    if let Some(value_type) = prop.param("VALUE")
        && !value_type.eq_ignore_ascii_case("DATE")
        && !value_type.eq_ignore_ascii_case("DATE-TIME")
    {
        bail!(
            "iCalendar line {line}: {what} declares VALUE={value_type}, which this codec does not \
             model (DATE and DATE-TIME are)."
        );
    }

    if date_value || (!raw.contains('T') && raw.len() == 8) {
        // RFC 5545 section 3.3.4, on the DATE value type: "The 'TZID' property
        // parameter MUST NOT be applied to DATE properties." Refused rather
        // than ignored: ignoring it would pick a zone for a value the feed
        // declared zone-free, which is exactly the silent choice this codec
        // refuses everywhere else.
        if let Some(tzid) = prop.param("TZID") {
            bail!(
                "iCalendar line {line}: {what} '{raw}' is a DATE value carrying TZID '{tzid}'. \
                 RFC 5545 section 3.3.4 forbids a TZID on a DATE — an all-day value has no zone."
            );
        }
        let date = NaiveDate::parse_from_str(raw, "%Y%m%d").map_err(|e| {
            anyhow::anyhow!("iCalendar line {line}: {what} '{raw}' is not a valid DATE: {e}")
        })?;
        let tz = calendar_tz.unwrap_or(chrono_tz::UTC);
        let naive = date
            .and_hms_opt(0, 0, 0)
            .expect("midnight always exists as a NaiveDateTime");
        let zoned = local_in(tz, naive, line, what)?;
        return Ok(Moment {
            all_day: true,
            line,
            raw: raw.to_string(),
            date,
            utc: zoned.with_timezone(&Utc),
            zoned: zoned.with_timezone(&RruleTz::from(tz)),
            tz_name: tz.name().to_string(),
        });
    }

    // A UTC DATE-TIME, marked by its trailing `Z`.
    if let Some(stripped) = raw.strip_suffix('Z') {
        let naive = parse_naive(stripped, line, what, raw)?;
        let utc = Utc.from_utc_datetime(&naive);
        return Ok(Moment {
            all_day: false,
            line,
            raw: raw.to_string(),
            date: utc.date_naive(),
            utc,
            zoned: utc.with_timezone(&RruleTz::from(Utc)),
            tz_name: "UTC".to_string(),
        });
    }

    // Either TZID-qualified, or floating (in which case only the calendar's own
    // timezone gives it an instant).
    let naive = parse_naive(raw, line, what, raw)?;
    let tz = match prop.param("TZID") {
        Some(tzid) => tzid.parse::<ChronoTz>().map_err(|_| {
            anyhow::anyhow!(
                "iCalendar line {line}: {what} carries TZID '{tzid}', which is not an IANA \
                 timezone name this build can resolve. The VTIMEZONE block's offsets are not \
                 replayed in its place: guessing a zone from a pair of offsets is how a wrong \
                 start time gets stored as if it were right."
            )
        })?,
        None => calendar_tz.ok_or_else(|| {
            anyhow::anyhow!(
                "iCalendar line {line}: {what} '{raw}' is a floating local time and this calendar \
                 declares no X-WR-TIMEZONE, so its instant is undecidable."
            )
        })?,
    };
    let zoned = local_in(tz, naive, line, what)?;
    Ok(Moment {
        all_day: false,
        line,
        raw: raw.to_string(),
        date: zoned.with_timezone(&Utc).date_naive(),
        utc: zoned.with_timezone(&Utc),
        zoned: zoned.with_timezone(&RruleTz::from(tz)),
        tz_name: tz.name().to_string(),
    })
}

fn parse_naive(value: &str, line: usize, what: &str, raw: &str) -> Result<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").map_err(|e| {
        anyhow::anyhow!("iCalendar line {line}: {what} '{raw}' is not a valid DATE-TIME: {e}")
    })
}

/// A wall-clock reading in `tz` into an instant. An hour that occurs twice
/// (the end of daylight saving) takes the FIRST, as RFC 5545 says; an hour that
/// does not occur at all (the start of it) has no instant and is refused.
fn local_in(
    tz: ChronoTz,
    naive: NaiveDateTime,
    line: usize,
    what: &str,
) -> Result<DateTime<ChronoTz>> {
    tz.from_local_datetime(&naive).earliest().ok_or_else(|| {
        anyhow::anyhow!(
            "iCalendar line {line}: {what} {naive} does not exist in {tz} (it falls in a \
             daylight-saving gap), so it has no instant."
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A FIXED clock, so every instant below is arithmetic rather than a
    /// lookup: a test whose expectation moves with the wall clock rots.
    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 16, 12, 0, 0).unwrap()
    }

    fn decode_ok(body: &str) -> Vec<serde_json::Value> {
        decode(body, IcsWindow::default(), now())
            .unwrap_or_else(|e| panic!("expected a decode, got: {e:#}"))
    }

    fn decode_err(body: &str) -> String {
        format!(
            "{:#}",
            decode(body, IcsWindow::default(), now()).expect_err("expected a refusal")
        )
    }

    fn ids(rows: &[serde_json::Value]) -> Vec<String> {
        let mut out: Vec<String> = rows
            .iter()
            .map(|r| r["id"].as_str().expect("every row has an id").to_string())
            .collect();
        out.sort();
        out
    }

    /// The whole document for one VEVENT, with a calendar-level timezone.
    fn cal(tz: &str, vevent: &str) -> String {
        format!(
            "BEGIN:VCALENDAR\nVERSION:2.0\nX-WR-TIMEZONE:{tz}\nBEGIN:VEVENT\n{vevent}\nEND:VEVENT\nEND:VCALENDAR\n"
        )
    }

    const DTSTAMP: &str = "DTSTAMP:20260101T000000Z";

    // --- time resolution ---------------------------------------------------

    #[test]
    fn a_tzid_datetime_resolves_through_the_zone_not_an_offset() {
        let rows = decode_ok(&cal(
            "Europe/Berlin",
            &format!(
                "UID:summer@example.com\nDTSTART;TZID=Europe/Berlin:20260916T100000\nDTEND;TZID=Europe/Berlin:20260916T110000\n{DTSTAMP}\nSUMMARY:Summer"
            ),
        ));
        assert_eq!(rows.len(), 1);
        // September is CEST (+02:00), so 10:00 local is 08:00Z.
        assert_eq!(rows[0]["start"], "2026-09-16T08:00:00Z");
        assert_eq!(rows[0]["end"], "2026-09-16T09:00:00Z");
        assert_eq!(rows[0]["tzid"], "Europe/Berlin");
        assert_eq!(rows[0]["all_day"], 0);
    }

    #[test]
    fn a_winter_tzid_datetime_uses_the_winter_offset() {
        let rows = decode_ok(&cal(
            "Europe/Berlin",
            &format!(
                "UID:winter@example.com\nDTSTART;TZID=Europe/Berlin:20261216T100000\n{DTSTAMP}"
            ),
        ));
        // December is CET (+01:00), so 10:00 local is 09:00Z. A decoder that
        // replayed a VTIMEZONE offset pair would get one of the two seasons
        // wrong for every event in the other.
        assert_eq!(rows[0]["start"], "2026-12-16T09:00:00Z");
    }

    #[test]
    fn a_utc_datetime_keeps_its_instant_whatever_the_calendar_timezone() {
        let rows = decode_ok(&cal(
            "Europe/Berlin",
            &format!("UID:utc@example.com\nDTSTART:20261005T120000Z\n{DTSTAMP}"),
        ));
        assert_eq!(rows[0]["start"], "2026-10-05T12:00:00Z");
        assert_eq!(rows[0]["tzid"], "UTC");
    }

    /// The date of an all-day value is the date AS WRITTEN. Midnight in Berlin
    /// is 22:00Z the previous day, so deriving it from the instant moves every
    /// all-day event back a day.
    #[test]
    fn an_all_day_date_is_the_date_as_written_not_its_utc_date() {
        let rows = decode_ok(&cal(
            "Europe/Berlin",
            &format!(
                "UID:allday@example.com\nDTSTART;VALUE=DATE:20261003\nDTEND;VALUE=DATE:20261004\n{DTSTAMP}"
            ),
        ));
        assert_eq!(rows[0]["start"], "2026-10-03");
        assert_eq!(rows[0]["end"], "2026-10-04");
        assert_eq!(rows[0]["all_day"], 1);
    }

    #[test]
    fn a_floating_time_uses_the_calendars_own_timezone() {
        let rows = decode_ok(&cal(
            "Europe/Berlin",
            &format!("UID:float@example.com\nDTSTART:20260916T100000\n{DTSTAMP}"),
        ));
        assert_eq!(rows[0]["start"], "2026-09-16T08:00:00Z");
        assert_eq!(rows[0]["tzid"], "Europe/Berlin");
    }

    #[test]
    fn a_floating_time_with_no_calendar_timezone_is_refused() {
        let why = decode_err(
            "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:f@example.com\nDTSTART:20260916T100000\nEND:VEVENT\nEND:VCALENDAR\n",
        );
        assert!(why.contains("floating"), "{why}");
    }

    // --- recurrence --------------------------------------------------------

    fn weekly(extra: &str) -> String {
        cal(
            "UTC",
            &format!(
                "UID:weekly@example.com\nDTSTART:20260916T100000Z\nDTEND:20260916T110000Z\nRRULE:FREQ=WEEKLY;COUNT=3\n{DTSTAMP}\nSUMMARY:Weekly\n{extra}"
            ),
        )
    }

    #[test]
    fn a_rule_with_a_count_expands_to_its_occurrences() {
        // 09-16, 09-23, 09-30 at 10:00Z.
        assert_eq!(
            ids(&decode_ok(&weekly(""))),
            vec![
                "weekly@example.com@2026-09-16T10:00:00Z",
                "weekly@example.com@2026-09-23T10:00:00Z",
                "weekly@example.com@2026-09-30T10:00:00Z",
            ]
        );
    }

    /// The COMMON shape in a real feed: a rule with no COUNT and no UNTIL,
    /// bounded only by the window. This is also the case that would break if a
    /// window bound were mistaken for hitting the occurrence cap.
    #[test]
    fn an_unbounded_rule_expands_only_inside_the_window() {
        let body = cal(
            "UTC",
            &format!(
                "UID:forever@example.com\nDTSTART:20260916T100000Z\nDTEND:20260916T110000Z\nRRULE:FREQ=WEEKLY\n{DTSTAMP}"
            ),
        );
        let rows = decode_ok(&body);
        assert!(
            (50..=60).contains(&rows.len()),
            "a weekly series over a 30d-back/365d-forward window is ~56 occurrences, got {}",
            rows.len()
        );
        let mut instants: Vec<&str> = rows.iter().map(|r| r["start"].as_str().unwrap()).collect();
        instants.sort();
        let window_start = now() - chrono::Duration::days(30);
        let window_end = now() + chrono::Duration::days(365);
        assert!(
            instants.first().unwrap() >= &rfc3339(window_start).as_str(),
            "the first occurrence must be inside the window, got {}",
            instants.first().unwrap()
        );
        assert!(
            instants.last().unwrap() <= &rfc3339(window_end).as_str(),
            "the last occurrence must be inside the window, got {}",
            instants.last().unwrap()
        );
        // And every occurrence keeps the event's length.
        assert_eq!(rows[0]["end"], "2026-09-16T11:00:00Z");
    }

    #[test]
    fn an_occurrence_before_the_window_is_not_replicated() {
        // A rule whose every occurrence is more than 30 days back.
        let body = cal(
            "UTC",
            &format!(
                "UID:past@example.com\nDTSTART:20250101T100000Z\nRRULE:FREQ=WEEKLY;COUNT=3\n{DTSTAMP}"
            ),
        );
        assert!(
            decode_ok(&body).is_empty(),
            "occurrences outside the window are not replicated"
        );
    }

    #[test]
    fn an_exdate_removes_its_occurrence() {
        let body = weekly("EXDATE:20260923T100000Z");
        assert_eq!(
            ids(&decode_ok(&body)),
            vec![
                "weekly@example.com@2026-09-16T10:00:00Z",
                "weekly@example.com@2026-09-30T10:00:00Z",
            ]
        );
    }

    #[test]
    fn an_exdate_list_removes_every_one_it_names() {
        let body = weekly("EXDATE:20260923T100000Z,20260930T100000Z");
        assert_eq!(
            ids(&decode_ok(&body)),
            vec!["weekly@example.com@2026-09-16T10:00:00Z"]
        );
    }

    /// An override lands on the SAME key as the occurrence it names, so under
    /// replace-scope it takes that occurrence's place, carrying its own times.
    #[test]
    fn an_override_replaces_the_occurrence_it_names() {
        let body = format!(
            "BEGIN:VCALENDAR\nVERSION:2.0\nX-WR-TIMEZONE:UTC\n\
             BEGIN:VEVENT\nUID:weekly@example.com\nDTSTART:20260916T100000Z\n\
             DTEND:20260916T110000Z\nRRULE:FREQ=WEEKLY;COUNT=3\n{DTSTAMP}\n\
             SUMMARY:Weekly\nEND:VEVENT\n\
             BEGIN:VEVENT\nUID:weekly@example.com\nDTSTART:20260930T140000Z\n\
             DTEND:20260930T153000Z\nRECURRENCE-ID:20260930T100000Z\n{DTSTAMP}\n\
             SUMMARY:Moved\nEND:VEVENT\nEND:VCALENDAR\n"
        );
        let rows = decode_ok(&body);
        assert_eq!(
            ids(&rows),
            vec![
                "weekly@example.com@2026-09-16T10:00:00Z",
                "weekly@example.com@2026-09-23T10:00:00Z",
                "weekly@example.com@2026-09-30T10:00:00Z",
            ],
            "the override replaces its occurrence rather than joining it"
        );
        let overridden = rows
            .iter()
            .find(|r| r["id"] == "weekly@example.com@2026-09-30T10:00:00Z")
            .expect("the override's row");
        assert_eq!(
            overridden["start"], "2026-09-30T14:00:00Z",
            "the override carries its own moved start"
        );
        assert_eq!(overridden["end"], "2026-09-30T15:30:00Z");
        assert_eq!(overridden["summary"], "Moved");
        assert_eq!(overridden["recurrence_id"], "2026-09-30T10:00:00Z");
    }

    #[test]
    fn an_override_with_no_master_still_emits_its_row() {
        let body = cal(
            "UTC",
            &format!(
                "UID:orphan@example.com\nDTSTART:20261001T140000Z\nRECURRENCE-ID:20261001T100000Z\n{DTSTAMP}\nSUMMARY:Only the override"
            ),
        );
        let rows = decode_ok(&body);
        assert_eq!(ids(&rows), vec!["orphan@example.com@2026-10-01T10:00:00Z"]);
        assert_eq!(rows[0]["start"], "2026-10-01T14:00:00Z");
    }

    /// Reaching the occurrence bound FAILS LOUD. Truncating would read as a
    /// deletion of every occurrence past the bound.
    #[test]
    fn too_many_occurrences_in_the_window_fails_loud() {
        let body = cal(
            "UTC",
            &format!(
                "UID:eek@example.com\nDTSTART:20260916T100000Z\nRRULE:FREQ=MINUTELY\n{DTSTAMP}"
            ),
        );
        let why = decode_err(&body);
        assert!(why.contains("eek@example.com"), "{why}");
        assert!(why.contains("ics_window"), "{why}");
    }

    // --- refusals ----------------------------------------------------------

    #[test]
    fn a_vtodo_is_refused_by_name() {
        let why = decode_err(
            "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VTODO\nUID:t@example.com\nSUMMARY:x\nEND:VTODO\nEND:VCALENDAR\n",
        );
        assert!(why.contains("VTODO"), "{why}");
    }

    #[test]
    fn a_vjournal_is_refused_by_name() {
        let why = decode_err(
            "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VJOURNAL\nUID:j@example.com\nEND:VJOURNAL\nEND:VCALENDAR\n",
        );
        assert!(why.contains("VJOURNAL"), "{why}");
    }

    #[test]
    fn a_non_iana_tzid_is_refused_and_says_the_block_is_not_replayed() {
        let why = decode_err(&cal(
            "UTC",
            "UID:win@example.com\nDTSTART;TZID=W. Europe Standard Time:20260916T100000\nDTSTAMP:x",
        ));
        assert!(why.contains("W. Europe Standard Time"), "{why}");
        assert!(why.contains("IANA"), "{why}");
    }

    #[test]
    fn a_duration_is_refused_rather_than_guessed() {
        let why = decode_err(&cal(
            "UTC",
            &format!("UID:d@example.com\nDTSTART:20260916T100000Z\nDURATION:PT1H\n{DTSTAMP}"),
        ));
        assert!(why.contains("DURATION"), "{why}");
    }

    #[test]
    fn an_unknown_value_type_is_refused() {
        let why = decode_err(&cal(
            "UTC",
            &format!("UID:v@example.com\nDTSTART;VALUE=PERIOD:20260916T100000Z\n{DTSTAMP}"),
        ));
        assert!(why.contains("VALUE=PERIOD"), "{why}");
    }

    #[test]
    fn a_vevent_with_no_uid_or_no_dtstart_is_refused_with_its_line() {
        let why = decode_err(&cal("UTC", &format!("DTSTART:20260916T100000Z\n{DTSTAMP}")));
        assert!(why.contains("UID"), "{why}");
        assert!(
            why.contains("line 4"),
            "the refusal must name a line a reader can open: {why}"
        );

        let why = decode_err(&cal(
            "UTC",
            &format!("UID:nodtstart@example.com\n{DTSTAMP}"),
        ));
        assert!(why.contains("DTSTART"), "{why}");
    }

    #[test]
    fn two_rrules_on_one_vevent_are_refused() {
        let why = decode_err(&cal(
            "UTC",
            &format!(
                "UID:two@example.com\nDTSTART:20260916T100000Z\nRRULE:FREQ=WEEKLY\nRRULE:FREQ=DAILY\n{DTSTAMP}"
            ),
        ));
        assert!(why.contains("more than one RRULE"), "{why}");
    }

    #[test]
    fn a_malformed_document_is_refused_rather_than_read_as_empty() {
        assert!(
            decode(
                "this is not iCalendar at all\n",
                IcsWindow::default(),
                now()
            )
            .is_err(),
            "a body that is not iCalendar must be REFUSED, never read as an empty calendar"
        );
        assert!(
            decode_err("BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:x@example.com\n")
                .contains("never closed")
        );
        assert!(
            decode_err("BEGIN:VCALENDAR\nVERSION:2.0\nEND:VCALENDAR\nEND:VCALENDAR\n")
                .contains("no matching BEGIN")
        );
        assert!(
            decode_err("BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:x@example.com\nEND:VTODO\nEND:VCALENDAR\n")
                .contains("closes BEGIN:VEVENT")
        );
    }

    // --- TEXT and folding --------------------------------------------------

    #[test]
    fn text_escapes_are_unescaped_and_a_folded_line_is_rejoined() {
        let rows = decode_ok(&cal(
            "UTC",
            &format!(
                "UID:text@example.com\nDTSTART:20260916T100000Z\n{DTSTAMP}\nSUMMARY:a\\, b\\; c\\\\ d\\ne\nDESCRIPTION:first half\n  second half"
            ),
        ));
        assert_eq!(rows[0]["summary"], "a, b; c\\ d\ne");
        assert_eq!(rows[0]["description"], "first half second half");
    }

    #[test]
    fn a_valarm_inside_a_vevent_is_ignored_rather_than_refused() {
        let rows = decode_ok(&cal(
            "UTC",
            &format!(
                "UID:alarm@example.com\nDTSTART:20260916T100000Z\n{DTSTAMP}\nBEGIN:VALARM\nACTION:DISPLAY\nTRIGGER:-PT15M\nEND:VALARM"
            ),
        ));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "alarm@example.com");
    }

    #[test]
    fn an_empty_calendar_is_legitimate() {
        assert!(decode_ok("BEGIN:VCALENDAR\nVERSION:2.0\nEND:VCALENDAR\n").is_empty());
    }

    // --- the delta findings -------------------------------------------------

    /// RFC 5545 section 3.1: a fold is a CRLF followed by "a single white space
    /// character (space or horizontal tab)". The marker is stripped on
    /// unfolding; the space after the tab in this fixture belongs to the TEXT.
    #[test]
    fn a_tab_folded_line_is_rejoined() {
        let body = "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:tab@example.com\n\
                    DTSTART:20260916T100000Z\nDTSTAMP:20260101T000000Z\n\
                    SUMMARY:first half\n\t second half\nEND:VEVENT\nEND:VCALENDAR\n";
        assert_eq!(decode_ok(body)[0]["summary"], "first half second half");
    }

    /// RFC 5545 section 3.3.4, on the DATE value type: "The 'TZID' property
    /// parameter MUST NOT be applied to DATE properties." Refused rather than
    /// ignored, because ignoring it means silently choosing a zone for a value
    /// the feed said was zone-free.
    #[test]
    fn a_tzid_on_a_date_value_is_refused() {
        let why = decode_err(&cal(
            "UTC",
            &format!(
                "UID:zoneddate@example.com\nDTSTART;VALUE=DATE;TZID=Europe/Berlin:20261003\n{DTSTAMP}"
            ),
        ));
        assert!(why.contains("TZID"), "{why}");
        assert!(
            why.contains("3.3.4"),
            "the refusal must cite the section that forbids it: {why}"
        );
    }

    /// An all-day series excludes an all-day EXDATE by LOCAL DATE, so the two
    /// meet whatever offset the zone carries.
    #[test]
    fn an_all_day_series_excludes_an_all_day_exdate_by_local_date() {
        let body = cal(
            "UTC",
            &format!(
                "UID:alldayseries@example.com\nDTSTART;VALUE=DATE:20261003\n\
                 RRULE:FREQ=WEEKLY;COUNT=3\nEXDATE;VALUE=DATE:20261010\n{DTSTAMP}"
            ),
        );
        assert_eq!(
            ids(&decode_ok(&body)),
            vec![
                "alldayseries@example.com@2026-10-03T00:00:00Z",
                "alldayseries@example.com@2026-10-17T00:00:00Z",
            ]
        );
    }

    /// An EXDATE that names an instant the series does not generate excludes
    /// nothing. Silent, that is a feed claiming six occurrences and delivering
    /// five with nothing said.
    #[test]
    fn an_exdate_that_names_no_occurrence_is_refused() {
        // The series is Wednesdays at 10:00Z; this names a Thursday.
        let why = decode_err(&weekly("EXDATE:20260917T100000Z"));
        assert!(why.contains("EXDATE"), "{why}");
        assert!(
            why.contains("20260917T100000Z") || why.contains("2026-09-17"),
            "the refusal must name the EXDATE it could not use: {why}"
        );
    }

    /// The window bounds EVERY row, not only an expansion: "the window is the
    /// replica's BOUND" is what the sidecar documentation promises.
    #[test]
    fn a_one_off_event_outside_the_window_is_not_replicated() {
        let body = cal(
            "UTC",
            &format!("UID:old@example.com\nDTSTART:20250101T100000Z\n{DTSTAMP}"),
        );
        assert!(
            decode_ok(&body).is_empty(),
            "a one-off event years before the window is not replicated either"
        );
    }

    #[test]
    fn a_one_off_event_inside_the_window_is_replicated() {
        let body = cal(
            "UTC",
            &format!("UID:soon@example.com\nDTSTART:20260920T100000Z\n{DTSTAMP}"),
        );
        assert_eq!(ids(&decode_ok(&body)), vec!["soon@example.com"]);
    }

    /// The first DATE the default window covers, in `tz`. An all-day row is
    /// judged against this, and its midnight INSTANT is normally the day before
    /// in UTC — which is why the judgement cannot be made on instants alone.
    fn window_start_day(tz: ChronoTz) -> NaiveDate {
        let back = IcsWindow::default().back.duration().as_secs() as i64;
        (now() - chrono::Duration::seconds(back))
            .with_timezone(&tz)
            .date_naive()
    }

    /// An EXDATE outside the window has nothing here to have excluded, so it
    /// says nothing about the feed and must NOT be refused. Refusing it would
    /// reject every calendar whose series predates the replica.
    #[test]
    fn an_exdate_outside_the_window_is_not_judged() {
        let rows = decode_ok(&weekly("EXDATE:20200101T100000Z"));
        assert_eq!(
            rows.len(),
            3,
            "an EXDATE far outside the window excludes nothing here and must not be judged"
        );
    }

    /// An all-day occurrence ON the window's first day is INSIDE the window (by
    /// its DATE), so it must reach the replica. Its midnight is the previous
    /// day in UTC, so a pre-filter that asked for an instant range would drop
    /// it before the date rule ever saw it.
    #[test]
    fn an_all_day_series_keeps_its_occurrence_on_the_window_start_day() {
        let start_day = window_start_day(chrono_tz::Europe::Berlin);
        assert_eq!(
            start_day.to_string(),
            "2026-08-17",
            "this test pins the window's first DAY; if the default window moved, move this"
        );
        let body = cal(
            "Europe/Berlin",
            &format!(
                "UID:series@example.com\nDTSTART;VALUE=DATE:{}\n\
                 RRULE:FREQ=WEEKLY;COUNT=3\n{DTSTAMP}",
                start_day.format("%Y%m%d")
            ),
        );
        assert_eq!(
            ids(&decode_ok(&body)),
            vec![
                "series@example.com@2026-08-16T22:00:00Z",
                "series@example.com@2026-08-23T22:00:00Z",
                "series@example.com@2026-08-30T22:00:00Z",
            ],
            "all three occurrences are inside the window by DATE; the first is the window's start day"
        );
    }

    /// The same boundary, with an EXDATE on it: the occurrence is inside the
    /// window by its date, so the EXDATE matches something and is honoured
    /// rather than judged dead.
    #[test]
    fn an_all_day_exdate_on_the_window_start_day_that_matches_is_honored() {
        let start_day = window_start_day(chrono_tz::Europe::Berlin);
        let d = start_day.format("%Y%m%d");
        let body = cal(
            "Europe/Berlin",
            &format!(
                "UID:series@example.com\nDTSTART;VALUE=DATE:{d}\n\
                 RRULE:FREQ=WEEKLY;COUNT=3\nEXDATE;VALUE=DATE:{d}\n{DTSTAMP}"
            ),
        );
        assert_eq!(
            ids(&decode_ok(&body)),
            vec![
                "series@example.com@2026-08-23T22:00:00Z",
                "series@example.com@2026-08-30T22:00:00Z",
            ],
            "the occurrence on the window's first day is excluded, and the EXDATE is not judged dead"
        );
    }

    /// The negative half of the same boundary: an all-day EXDATE on the
    /// window's first day that matches NO occurrence is judged (it is inside
    /// the window by date) and refused. Judged by instant, its midnight would
    /// fall on the previous UTC day, look out of window, and pass in silence.
    #[test]
    fn an_all_day_exdate_on_the_window_start_day_that_matches_nothing_is_refused() {
        let start_day = window_start_day(chrono_tz::Europe::Berlin);
        let body = cal(
            "Europe/Berlin",
            &format!(
                "UID:series@example.com\nDTSTART;VALUE=DATE:{}\n\
                 RRULE:FREQ=WEEKLY;COUNT=3\nEXDATE;VALUE=DATE:{}\n{DTSTAMP}",
                (start_day + chrono::Duration::days(3)).format("%Y%m%d"),
                start_day.format("%Y%m%d")
            ),
        );
        let why = decode_err(&body);
        assert!(why.contains("EXDATE"), "{why}");
        assert!(
            why.contains("20260817"),
            "the EXDATE sits on the window's FIRST day, so it is judged; naming it is how the \
             reader sees which one: {why}"
        );
    }
}
