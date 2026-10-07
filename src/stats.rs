use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{Datelike, Duration, NaiveDate, TimeZone, Timelike};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Outcome {
    Done,
    Cancelled,
    Failed,
}

/// One model request. Only metadata is recorded, never prompts or answers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Record {
    /// Unix seconds when the request started.
    pub(crate) ts: i64,
    /// Groups the requests of one user turn (tool rounds make several requests).
    pub(crate) turn: String,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    /// True when a token count was estimated because the provider did not report it.
    pub(crate) estimated: bool,
    pub(crate) duration_ms: u64,
    pub(crate) tool_calls: u32,
    pub(crate) outcome: Outcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Range {
    All,
    Days(u32),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ModelStats {
    pub(crate) model: String,
    pub(crate) tokens: u64,
    pub(crate) requests: u64,
    pub(crate) tokens_per_second: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Summary {
    pub(crate) total_input: u64,
    pub(crate) total_output: u64,
    pub(crate) estimated: bool,
    pub(crate) requests: u64,
    pub(crate) turns: u64,
    pub(crate) failed: u64,
    pub(crate) cancelled: u64,
    pub(crate) models: Vec<ModelStats>,
    pub(crate) active_days: u32,
    pub(crate) span_days: u32,
    pub(crate) current_streak: u32,
    pub(crate) longest_streak: u32,
    pub(crate) most_active_day: Option<(NaiveDate, u64)>,
    pub(crate) peak_hour: Option<u32>,
    pub(crate) longest_turn_ms: u64,
}

impl Summary {
    pub(crate) fn total_tokens(&self) -> u64 {
        self.total_input + self.total_output
    }

    pub(crate) fn favorite(&self) -> Option<&str> {
        self.models.first().map(|model| model.model.as_str())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HeatGrid {
    /// One column per week, Monday first; `None` for days outside the shown window.
    pub(crate) columns: Vec<[Option<u8>; 7]>,
    pub(crate) first_monday: NaiveDate,
}

#[cfg(not(test))]
pub(crate) fn stats_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not locate the home directory")?;
    Ok(home.join(".coolcode").join("stats.jsonl"))
}

// Tests exercise recording; keep them away from the user's real history.
#[cfg(test)]
pub(crate) fn stats_path() -> Result<PathBuf> {
    Ok(std::env::temp_dir()
        .join(format!("harness-test-{}", std::process::id()))
        .join("stats.jsonl"))
}

pub(crate) fn append_to(path: &Path, record: &Record) -> Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let line = serde_json::to_string(record).context("serializing a usage record")?;
    writeln!(file, "{line}").with_context(|| format!("writing {}", path.display()))
}

/// Reads every readable record; a missing file is empty and damaged lines are skipped.
pub(crate) fn load_from(path: &Path) -> Vec<Record> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Record>(line).ok())
        .collect()
}

pub(crate) fn clear_at(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("deleting {}", path.display())),
    }
}

/// What was observed about one model request, before token estimates are filled in.
pub(crate) struct Facts<'a> {
    pub(crate) started_ts: i64,
    pub(crate) turn: &'a str,
    pub(crate) provider: &'a str,
    pub(crate) model: &'a str,
    pub(crate) reported_input: Option<u64>,
    pub(crate) reported_output: Option<u64>,
    pub(crate) input_chars: usize,
    pub(crate) output_chars: usize,
    pub(crate) duration_ms: u64,
    pub(crate) tool_calls: u32,
    pub(crate) outcome: Outcome,
}

/// Builds the record for a request. Missing token counts are estimated from text length (and
/// flagged), except for a request that failed before producing anything, which costs nothing.
pub(crate) fn build_record(facts: &Facts) -> Record {
    let estimate = |chars: usize| chars.div_ceil(4) as u64;
    // A failure that produced no text never reached the model, so nothing was consumed.
    let nothing_consumed = facts.outcome == Outcome::Failed && facts.output_chars == 0;
    let (input_tokens, input_estimated) = match facts.reported_input {
        Some(tokens) => (tokens, false),
        None if nothing_consumed => (0, false),
        None => (estimate(facts.input_chars), true),
    };
    let (output_tokens, output_estimated) = match facts.reported_output {
        Some(tokens) => (tokens, false),
        None if nothing_consumed => (0, false),
        None => (estimate(facts.output_chars), true),
    };
    Record {
        ts: facts.started_ts,
        turn: facts.turn.to_owned(),
        provider: facts.provider.to_owned(),
        model: facts.model.to_owned(),
        input_tokens,
        output_tokens,
        estimated: input_estimated || output_estimated,
        duration_ms: facts.duration_ms,
        tool_calls: facts.tool_calls,
        outcome: facts.outcome,
    }
}

/// Records one request; failures are ignored so stats can never interrupt a chat.
pub(crate) fn record(record: &Record) {
    if let Ok(path) = stats_path() {
        let _ = append_to(&path, record);
    }
}

pub(crate) fn load() -> Vec<Record> {
    stats_path()
        .map(|path| load_from(&path))
        .unwrap_or_default()
}

pub(crate) fn clear() -> Result<()> {
    clear_at(&stats_path()?)
}

fn local_date<Tz: TimeZone>(tz: &Tz, ts: i64) -> Option<(NaiveDate, u32)> {
    let moment = tz.timestamp_opt(ts, 0).single()?;
    Some((moment.date_naive(), moment.hour()))
}

#[derive(Default)]
struct ModelTotals {
    tokens: u64,
    requests: u64,
    output_with_time: u64,
    millis: u64,
}

pub(crate) fn summarize<Tz: TimeZone>(
    records: &[Record],
    range: Range,
    tz: &Tz,
    now_ts: i64,
) -> Summary {
    let Some((today, _)) = local_date(tz, now_ts) else {
        return Summary::default();
    };
    let cutoff = match range {
        Range::All => None,
        Range::Days(days) => Some(today - Duration::days(i64::from(days.max(1)) - 1)),
    };
    let mut summary = Summary::default();
    let mut models: HashMap<&str, ModelTotals> = HashMap::new();
    let mut day_tokens: BTreeMap<NaiveDate, u64> = BTreeMap::new();
    let mut hour_requests = [0u64; 24];
    let mut turn_spans: HashMap<&str, (i64, i64)> = HashMap::new();
    for record in records {
        let Some((date, hour)) = local_date(tz, record.ts) else {
            continue;
        };
        if cutoff.is_some_and(|cutoff| date < cutoff) {
            continue;
        }
        let tokens = record.input_tokens + record.output_tokens;
        summary.total_input += record.input_tokens;
        summary.total_output += record.output_tokens;
        summary.estimated |= record.estimated;
        summary.requests += 1;
        match record.outcome {
            Outcome::Done => {}
            Outcome::Cancelled => summary.cancelled += 1,
            Outcome::Failed => summary.failed += 1,
        }
        let totals = models.entry(record.model.as_str()).or_default();
        totals.tokens += tokens;
        totals.requests += 1;
        if record.duration_ms > 0 {
            totals.output_with_time += record.output_tokens;
            totals.millis += record.duration_ms;
        }
        *day_tokens.entry(date).or_default() += tokens;
        hour_requests[hour as usize] += 1;
        let start = record.ts * 1000;
        let end = start + record.duration_ms as i64;
        let span = turn_spans
            .entry(record.turn.as_str())
            .or_insert((start, end));
        span.0 = span.0.min(start);
        span.1 = span.1.max(end);
    }
    summary.turns = turn_spans.len() as u64;
    summary.longest_turn_ms = turn_spans
        .values()
        .map(|(start, end)| (end - start).max(0) as u64)
        .max()
        .unwrap_or(0);
    let mut model_list: Vec<ModelStats> = models
        .into_iter()
        .map(|(model, totals)| ModelStats {
            model: model.to_owned(),
            tokens: totals.tokens,
            requests: totals.requests,
            tokens_per_second: (totals.millis > 0)
                .then(|| totals.output_with_time as f64 / (totals.millis as f64 / 1000.0)),
        })
        .collect();
    model_list.sort_by(|a, b| {
        b.tokens
            .cmp(&a.tokens)
            .then(b.requests.cmp(&a.requests))
            .then(a.model.cmp(&b.model))
    });
    summary.models = model_list;
    summary.active_days = day_tokens.len() as u32;
    summary.most_active_day = day_tokens
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
        .map(|(date, tokens)| (*date, *tokens));
    summary.peak_hour = hour_requests
        .iter()
        .enumerate()
        .filter(|(_, count)| **count > 0)
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(&a.0)))
        .map(|(hour, _)| hour as u32);
    if let Some(first) = day_tokens.keys().next() {
        summary.span_days = ((today - *first).num_days() + 1).max(1) as u32;
    }
    let days: Vec<NaiveDate> = day_tokens.keys().copied().collect();
    let mut run = 0u32;
    let mut previous: Option<NaiveDate> = None;
    for day in &days {
        run = match previous {
            Some(before) if *day - before == Duration::days(1) => run + 1,
            _ => 1,
        };
        summary.longest_streak = summary.longest_streak.max(run);
        previous = Some(*day);
    }
    // The streak stays alive through a day with no usage yet; a full missed day ends it.
    let mut cursor = if day_tokens.contains_key(&today) {
        Some(today)
    } else {
        Some(today - Duration::days(1)).filter(|day| day_tokens.contains_key(day))
    };
    while let Some(day) = cursor {
        summary.current_streak += 1;
        cursor = Some(day - Duration::days(1)).filter(|before| day_tokens.contains_key(before));
    }
    summary
}

pub(crate) fn heat_grid<Tz: TimeZone>(
    records: &[Record],
    tz: &Tz,
    now_ts: i64,
    weeks: usize,
) -> HeatGrid {
    let weeks = weeks.max(1);
    let today = local_date(tz, now_ts)
        .map(|(date, _)| date)
        .unwrap_or_default();
    let this_monday = today - Duration::days(i64::from(today.weekday().num_days_from_monday()));
    let first_monday = this_monday - Duration::days(7 * (weeks as i64 - 1));
    let mut day_tokens: HashMap<NaiveDate, u64> = HashMap::new();
    for record in records {
        if let Some((date, _)) = local_date(tz, record.ts) {
            *day_tokens.entry(date).or_default() += record.input_tokens + record.output_tokens;
        }
    }
    let mut shown: Vec<u64> = day_tokens
        .iter()
        .filter(|(date, tokens)| **date >= first_monday && **date <= today && **tokens > 0)
        .map(|(_, tokens)| *tokens)
        .collect();
    shown.sort_unstable();
    let quartile = |fraction: f64| -> u64 {
        if shown.is_empty() {
            return 0;
        }
        shown[((shown.len() - 1) as f64 * fraction).floor() as usize]
    };
    let (q1, q2, q3) = (quartile(0.25), quartile(0.5), quartile(0.75));
    let columns = (0..weeks)
        .map(|week| {
            let mut column = [None; 7];
            for (offset, cell) in column.iter_mut().enumerate() {
                let date = first_monday + Duration::days(7 * week as i64 + offset as i64);
                if date > today {
                    continue;
                }
                let tokens = day_tokens.get(&date).copied().unwrap_or(0);
                *cell = Some(if tokens == 0 {
                    0
                } else {
                    1 + u8::from(tokens > q1) + u8::from(tokens > q2) + u8::from(tokens > q3)
                });
            }
            column
        })
        .collect();
    HeatGrid {
        columns,
        first_monday,
    }
}

/// A rough, playful size comparison; the book sizes are approximate token counts.
pub(crate) fn fun_fact(total_tokens: u64) -> Option<String> {
    const BOOKS: [(&str, u64); 4] = [
        ("a short story", 7_000),
        ("The Hobbit", 125_000),
        ("The Lord of the Rings", 800_000),
        ("the Harry Potter series", 1_450_000),
    ];
    let (name, size) = BOOKS.iter().rev().find(|(_, size)| total_tokens >= *size)?;
    Some(format!(
        "That's about {:.1}× {name} (a rough comparison).",
        total_tokens as f64 / *size as f64
    ))
}

fn trim_decimal(value: f64) -> String {
    let text = format!("{value:.1}");
    text.strip_suffix(".0").map(str::to_owned).unwrap_or(text)
}

pub(crate) fn compact_tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 999_950 {
        format!("{}k", trim_decimal(n as f64 / 1_000.0))
    } else {
        format!("{}M", trim_decimal(n as f64 / 1_000_000.0))
    }
}

pub(crate) fn format_duration(ms: u64) -> String {
    let seconds = ms / 1000;
    match ms {
        0..=9_999 => format!("{:.1}s", ms as f64 / 1000.0),
        10_000..=59_999 => format!("{seconds}s"),
        60_000..=3_599_999 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ => format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn tz() -> FixedOffset {
        FixedOffset::east_opt(3 * 3600).expect("offset")
    }

    fn at(day: &str, hour: u32) -> i64 {
        let date = NaiveDate::parse_from_str(day, "%Y-%m-%d").expect("date");
        tz().from_local_datetime(&date.and_hms_opt(hour, 0, 0).expect("time"))
            .single()
            .expect("local time")
            .timestamp()
    }

    #[allow(clippy::too_many_arguments)]
    fn rec(
        day: &str,
        hour: u32,
        model: &str,
        input: u64,
        output: u64,
        turn: &str,
        ms: u64,
    ) -> Record {
        Record {
            ts: at(day, hour),
            turn: turn.to_owned(),
            provider: "p".to_owned(),
            model: model.to_owned(),
            input_tokens: input,
            output_tokens: output,
            estimated: false,
            duration_ms: ms,
            tool_calls: 0,
            outcome: Outcome::Done,
        }
    }

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("harness-stats-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("stats.jsonl")
    }

    #[test]
    fn records_round_trip_through_a_jsonl_file() {
        let path = temp_file("roundtrip");
        let first = rec("2026-10-01", 9, "m1", 100, 20, "t1", 1500);
        let mut second = rec("2026-10-02", 10, "m2", 5, 6, "t2", 10);
        second.outcome = Outcome::Cancelled;
        append_to(&path, &first).expect("append");
        append_to(&path, &second).expect("append");
        assert_eq!(load_from(&path), vec![first, second]);
    }

    #[test]
    fn missing_files_are_empty_and_malformed_lines_are_skipped() {
        let path = temp_file("malformed");
        assert!(load_from(&path).is_empty());
        let good = rec("2026-10-01", 9, "m1", 1, 1, "t1", 1);
        append_to(&path, &good).expect("append");
        let mut text = std::fs::read_to_string(&path).expect("read");
        text.push_str("not json at all\n{\"ts\": \"wrong\"}\n\n");
        std::fs::write(&path, text).expect("write");
        append_to(&path, &good).expect("append");
        assert_eq!(load_from(&path).len(), 2);
    }

    #[test]
    fn clearing_removes_the_history_and_tolerates_a_missing_file() {
        let path = temp_file("clear");
        clear_at(&path).expect("nothing to clear");
        append_to(&path, &rec("2026-10-01", 9, "m1", 1, 1, "t1", 1)).expect("append");
        clear_at(&path).expect("clear");
        assert!(load_from(&path).is_empty());
    }

    #[test]
    fn totals_favorite_model_requests_and_turns() {
        let mut failed = rec("2026-10-02", 11, "small", 10, 0, "t2", 5);
        failed.outcome = Outcome::Failed;
        let mut guess = rec("2026-10-02", 12, "small", 10, 5, "t3", 5);
        guess.estimated = true;
        let records = vec![
            rec("2026-10-01", 9, "big", 1000, 200, "t1", 4000),
            rec("2026-10-01", 9, "big", 2000, 300, "t1", 3000),
            failed,
            guess,
        ];
        let summary = summarize(&records, Range::All, &tz(), at("2026-10-03", 12));
        assert_eq!(summary.total_input, 3020);
        assert_eq!(summary.total_output, 505);
        assert_eq!(summary.total_tokens(), 3525);
        assert_eq!(summary.requests, 4);
        assert_eq!(summary.turns, 3);
        assert_eq!(summary.failed, 1);
        assert!(summary.estimated);
        assert_eq!(summary.favorite(), Some("big"));
        assert_eq!(summary.models[0].tokens, 3500);
        assert_eq!(summary.models[0].requests, 2);
        assert_eq!(summary.models[1].model, "small");
    }

    #[test]
    fn model_speed_is_output_tokens_over_generation_time() {
        let records = vec![
            rec("2026-10-01", 9, "fast", 1000, 100, "t1", 2000),
            rec("2026-10-01", 9, "fast", 1000, 100, "t2", 2000),
            rec("2026-10-01", 9, "instant", 5, 5, "t3", 0),
        ];
        let summary = summarize(&records, Range::All, &tz(), at("2026-10-02", 0));
        let fast = summary
            .models
            .iter()
            .find(|m| m.model == "fast")
            .expect("fast");
        assert_eq!(fast.tokens_per_second, Some(50.0));
        let instant = summary
            .models
            .iter()
            .find(|m| m.model == "instant")
            .expect("instant");
        assert_eq!(instant.tokens_per_second, None);
    }

    #[test]
    fn ranges_keep_only_recent_local_days() {
        let records = vec![
            rec("2026-10-01", 9, "m", 1, 1, "t1", 1),
            rec("2026-10-05", 9, "m", 1, 1, "t2", 1),
            rec("2026-10-09", 9, "m", 1, 1, "t3", 1),
        ];
        let now = at("2026-10-10", 12);
        assert_eq!(summarize(&records, Range::All, &tz(), now).requests, 3);
        assert_eq!(summarize(&records, Range::Days(30), &tz(), now).requests, 3);
        // Seven days ending today: Oct 4 through Oct 10.
        assert_eq!(summarize(&records, Range::Days(7), &tz(), now).requests, 2);
        assert_eq!(summarize(&records, Range::Days(1), &tz(), now).requests, 0);
    }

    #[test]
    fn days_follow_the_local_timezone() {
        // 22:30 UTC on Oct 1 is 01:30 on Oct 2 at +03:00.
        let mut late = rec("2026-10-02", 1, "m", 1, 1, "t1", 1);
        late.ts += 30 * 60;
        // In UTC this request happened on Oct 1, so only the local timezone puts it on Oct 2.
        let utc_day = chrono::DateTime::from_timestamp(late.ts, 0)
            .expect("utc")
            .date_naive();
        assert_eq!(utc_day, NaiveDate::from_ymd_opt(2026, 10, 1).unwrap());
        let summary = summarize(&[late], Range::All, &tz(), at("2026-10-02", 12));
        let (day, _) = summary.most_active_day.expect("day");
        assert_eq!(day, NaiveDate::from_ymd_opt(2026, 10, 2).unwrap());
        assert_eq!(summary.peak_hour, Some(1));
    }

    #[test]
    fn streaks_count_consecutive_active_days() {
        let days = [
            "2026-10-01",
            "2026-10-02",
            "2026-10-03",
            "2026-10-05",
            "2026-10-06",
        ];
        let records: Vec<_> = days
            .iter()
            .enumerate()
            .map(|(i, day)| rec(day, 10, "m", 1, 1, &format!("t{i}"), 1))
            .collect();
        let on_the_6th = summarize(&records, Range::All, &tz(), at("2026-10-06", 20));
        assert_eq!(
            (on_the_6th.current_streak, on_the_6th.longest_streak),
            (2, 3)
        );
        assert_eq!(on_the_6th.active_days, 5);
        // Nothing yet today, but yesterday was active: the streak is still alive.
        let on_the_7th = summarize(&records, Range::All, &tz(), at("2026-10-07", 8));
        assert_eq!(on_the_7th.current_streak, 2);
        // A full missed day ends it.
        let on_the_8th = summarize(&records, Range::All, &tz(), at("2026-10-08", 8));
        assert_eq!(on_the_8th.current_streak, 0);
        assert_eq!(on_the_8th.longest_streak, 3);
        assert_eq!(on_the_6th.span_days, 6);
    }

    #[test]
    fn busiest_day_peak_hour_and_longest_turn() {
        let records = vec![
            rec("2026-10-01", 21, "m", 100, 100, "a", 1000),
            rec("2026-10-02", 21, "m", 500, 500, "b", 60_000),
            rec("2026-10-02", 21, "m", 500, 500, "b", 120_000),
            rec("2026-10-03", 8, "m", 10, 10, "c", 1000),
        ];
        let summary = summarize(&records, Range::All, &tz(), at("2026-10-04", 0));
        let (day, tokens) = summary.most_active_day.expect("busiest day");
        assert_eq!(day, NaiveDate::from_ymd_opt(2026, 10, 2).unwrap());
        assert_eq!(tokens, 2000);
        assert_eq!(summary.peak_hour, Some(21));
        // Turn "b" starts at 21:00 and its last request ends 120 s later.
        assert_eq!(summary.longest_turn_ms, 120_000);
    }

    #[test]
    fn an_empty_history_summarizes_to_zeroes() {
        let summary = summarize(&[], Range::All, &tz(), at("2026-10-01", 0));
        assert_eq!(summary, Summary::default());
        assert_eq!(summary.favorite(), None);
    }

    #[test]
    fn the_heatmap_has_week_columns_and_levels_by_usage() {
        let records = vec![
            rec("2026-10-05", 9, "m", 10, 0, "a", 1),
            rec("2026-10-06", 9, "m", 100, 0, "b", 1),
            rec("2026-10-07", 9, "m", 1000, 0, "c", 1),
            rec("2026-10-08", 9, "m", 10_000, 0, "d", 1),
        ];
        // Oct 7, 2026 is a Wednesday; the week starts on Monday Oct 5.
        let grid = heat_grid(&records, &tz(), at("2026-10-07", 12), 3);
        assert_eq!(grid.columns.len(), 3);
        assert_eq!(
            grid.first_monday,
            NaiveDate::from_ymd_opt(2026, 9, 21).unwrap()
        );
        let current = grid.columns[2];
        assert_eq!(current[0], Some(1)); // Mon Oct 5, least used
        assert!(current[1].unwrap() >= 1 && current[1].unwrap() <= 4);
        assert_eq!(current[2], Some(4)); // Wed Oct 7, today (1000 tokens)
        assert_eq!(current[3], None); // Thu Oct 8 is in the future
        assert_eq!(grid.columns[0][0], Some(0)); // an unused past day
    }

    #[test]
    fn the_fun_line_compares_usage_to_books() {
        assert_eq!(fun_fact(500), None);
        let hobbit = fun_fact(150_000).expect("hobbit");
        assert!(
            hobbit.contains("The Hobbit") && hobbit.contains("1.2×"),
            "{hobbit}"
        );
        let potter = fun_fact(2_900_000).expect("potter");
        assert!(
            potter.contains("Harry Potter") && potter.contains("2.0×"),
            "{potter}"
        );
    }

    fn facts<'a>(outcome: Outcome) -> Facts<'a> {
        Facts {
            started_ts: 1_790_000_000,
            turn: "turn-1",
            provider: "MultiAI",
            model: "deepseek/deepseek-v4-flash-free",
            reported_input: None,
            reported_output: None,
            input_chars: 400,
            output_chars: 40,
            duration_ms: 1500,
            tool_calls: 2,
            outcome,
        }
    }

    #[test]
    fn reported_token_counts_are_used_as_they_are() {
        let mut observed = facts(Outcome::Done);
        observed.reported_input = Some(1234);
        observed.reported_output = Some(56);
        let record = build_record(&observed);
        assert_eq!((record.input_tokens, record.output_tokens), (1234, 56));
        assert!(!record.estimated);
        assert_eq!(record.model, "deepseek/deepseek-v4-flash-free");
        assert_eq!(record.provider, "MultiAI");
        assert_eq!((record.tool_calls, record.duration_ms), (2, 1500));
        assert_eq!(record.turn, "turn-1");
    }

    #[test]
    fn missing_counts_are_estimated_from_text_length_and_flagged() {
        let record = build_record(&facts(Outcome::Done));
        assert_eq!((record.input_tokens, record.output_tokens), (100, 10));
        assert!(record.estimated);
        let mut partly = facts(Outcome::Cancelled);
        partly.reported_input = Some(7);
        let record = build_record(&partly);
        assert_eq!((record.input_tokens, record.output_tokens), (7, 10));
        assert!(record.estimated);
    }

    #[test]
    fn a_request_that_failed_before_producing_anything_costs_nothing() {
        let mut nothing = facts(Outcome::Failed);
        nothing.output_chars = 0;
        let record = build_record(&nothing);
        assert_eq!((record.input_tokens, record.output_tokens), (0, 0));
        assert!(!record.estimated);
        assert_eq!(record.outcome, Outcome::Failed);
        // A failure after some text streamed did consume tokens.
        let record = build_record(&facts(Outcome::Failed));
        assert!(record.output_tokens > 0 && record.estimated);
    }

    #[test]
    fn numbers_and_durations_are_compact() {
        assert_eq!(compact_tokens(950), "950");
        assert_eq!(compact_tokens(1_500), "1.5k");
        assert_eq!(compact_tokens(980_000), "980k");
        assert_eq!(compact_tokens(1_234_567), "1.2M");
        assert_eq!(format_duration(900), "0.9s");
        assert_eq!(format_duration(45_000), "45s");
        assert_eq!(format_duration(252_000), "4m 12s");
        assert_eq!(format_duration(3_900_000), "1h 5m");
    }
}
