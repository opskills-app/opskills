//! Scheduled runs: cron expressions in a time zone, and one-time runs.
//!
//! The scheduler wakes every few seconds and starts every run that came due since it
//! last looked. Times that passed while the server wasn't running are skipped, not
//! replayed. The last time each schedule fired is stored, so a restart never fires
//! the same time twice.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::Tz;
use croner::Cron;
use serde::{Deserialize, Serialize};

use crate::model::{Runbook, Schedule, now_ms};
use crate::web::AppState;

/// How often the scheduler looks for due runs.
pub const TICK: Duration = Duration::from_secs(5);

/// Parse a standard 5-field cron expression (minute, hour, day of month, month, day of
/// week) or a name like `@daily`.
pub fn parse_cron(expr: &str) -> Result<Cron, String> {
    let e = expr.trim();
    let named = e.starts_with('@');
    if named && e.eq_ignore_ascii_case("@reboot") {
        return Err("@reboot isn't supported; use a time-based schedule.".into());
    }
    if !named && e.split_whitespace().count() != 5 {
        return Err(format!("{e:?} needs 5 fields: minute, hour, day of month, month and day of week, like \"0 3 * * 1-5\"."));
    }
    e.parse::<Cron>().map_err(|err| format!("{e:?} is not a valid cron expression: {err}."))
}

pub fn parse_tz(name: &str) -> Result<Tz, String> {
    name.trim().parse::<Tz>().map_err(|_| format!("{name:?} is not a time zone. Use a name like UTC, Europe/Berlin or America/New_York."))
}

/// Up to `n` times (epoch ms) when `cron` fires in `tz`, strictly after `after_ms`.
pub fn next_times(cron: &Cron, tz: Tz, after_ms: u64, n: usize) -> Vec<u64> {
    let Some(start) = Utc.timestamp_millis_opt(after_ms as i64).single() else { return vec![] };
    let mut t: DateTime<Tz> = start.with_timezone(&tz);
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        match cron.find_next_occurrence(&t, false) {
            Ok(next) => {
                out.push(next.timestamp_millis() as u64);
                t = next;
            }
            Err(_) => break,
        }
    }
    out
}

/// The next times a schedule fires after `after_ms`.
pub fn upcoming(s: &Schedule, after_ms: u64, n: usize) -> Vec<u64> {
    if !s.enabled {
        return vec![];
    }
    match (&s.cron, s.at) {
        (Some(expr), _) => match (parse_cron(expr), parse_tz(&s.timezone)) {
            (Ok(cron), Ok(tz)) => next_times(&cron, tz, after_ms, n),
            _ => vec![],
        },
        (None, Some(at)) if at > after_ms => vec![at],
        _ => vec![],
    }
}

/// The time a schedule should fire for now: the latest time it was due that is after
/// `after` and not after `now`. Older missed times are skipped, so a scheduler that
/// fell behind fires once, not once per missed time.
pub fn due_slot(s: &Schedule, after: u64, now: u64) -> Option<u64> {
    if !s.enabled {
        return None;
    }
    match (&s.cron, s.at) {
        (Some(expr), _) => {
            let (cron, tz) = (parse_cron(expr).ok()?, parse_tz(&s.timezone).ok()?);
            let now_tz = Utc.timestamp_millis_opt(now as i64).single()?.with_timezone(&tz);
            let latest = cron.find_previous_occurrence(&now_tz, true).ok()?.timestamp_millis() as u64;
            (latest > after).then_some(latest)
        }
        (None, Some(at)) => (at > after && at <= now).then_some(at),
        _ => None,
    }
}

/// What the scheduler remembers about one schedule.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ScheduleState {
    /// The scheduled time it last fired for (epoch ms), not the wall clock time.
    pub last_slot: Option<u64>,
    pub last_fired_at: Option<u64>,
    pub last_run_id: Option<String>,
    pub last_error: Option<String>,
}

pub fn state_key(runbook_id: &str, schedule_id: &str) -> String {
    format!("{runbook_id}/{schedule_id}")
}

/// Start every run that came due after `floor` and up to `now`. `floor` is when the
/// scheduler started, so times missed while the server was down are skipped.
/// Returns the ids of the runs it started.
pub fn tick(st: &Arc<AppState>, floor: u64, now: u64) -> Vec<String> {
    let mut started = Vec::new();
    let runbooks = match st.store.runbooks() {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "scheduler could not read runbooks");
            return started;
        }
    };
    for rb in runbooks {
        for s in rb.schedules.iter().filter(|s| s.enabled) {
            let key = state_key(&rb.id, &s.id);
            let mut state = st.store.schedule_state(&key).ok().flatten().unwrap_or_default();
            let after = floor.max(state.last_slot.unwrap_or(0));
            let Some(slot) = due_slot(s, after, now) else { continue };
            state.last_slot = Some(slot);
            state.last_fired_at = Some(now);
            match fire(st, &rb, s) {
                Ok(run_id) => {
                    tracing::info!(runbook = %rb.name, schedule = %s.id, run_id = %run_id, "scheduled run started");
                    state.last_run_id = Some(run_id.clone());
                    state.last_error = None;
                    started.push(run_id);
                }
                Err(msg) => {
                    tracing::warn!(runbook = %rb.name, schedule = %s.id, error = %msg, "scheduled run could not start");
                    state.last_error = Some(msg);
                }
            }
            if let Err(e) = st.store.put_schedule_state(&key, &state) {
                tracing::error!(error = %e, "saving schedule state failed");
            }
        }
    }
    started
}

fn fire(st: &Arc<AppState>, rb: &Runbook, s: &Schedule) -> Result<String, String> {
    let given = s.inputs.iter().map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone()))).collect();
    let by = if s.name.is_empty() { "schedule".to_string() } else { format!("schedule {}", s.name) };
    crate::api::start_runbook(st, rb, &given, &by, "schedule").map(|run| run.id).map_err(|e| e.detail())
}

/// Run the scheduler until the process ends.
pub fn spawn(st: Arc<AppState>) {
    let floor = now_ms();
    tokio::spawn(async move {
        let mut tick_timer = tokio::time::interval(TICK);
        loop {
            tick_timer.tick().await;
            let st = st.clone();
            // Starting runs touches the database; keep it off the async workers.
            let _ = tokio::task::spawn_blocking(move || tick(&st, floor, now_ms())).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    fn ms(y: i32, mo: u32, d: u32, h: u32, mi: u32, tz: Tz) -> u64 {
        tz.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap().timestamp_millis() as u64
    }

    #[test]
    fn cron_expressions_are_checked() {
        assert!(parse_cron("0 3 * * 1-5").is_ok());
        assert!(parse_cron("*/15 * * * *").is_ok());
        assert!(parse_cron("@daily").is_ok());
        for bad in ["", "0 3 * *", "0 0 3 * * *", "61 * * * *", "@reboot", "every day"] {
            assert!(parse_cron(bad).is_err(), "{bad}");
        }
        assert!(parse_tz("Europe/Berlin").is_ok());
        assert!(parse_tz("Mars/Olympus").is_err());
    }

    #[test]
    fn next_times_follow_the_time_zone_across_daylight_saving() {
        let berlin: Tz = "Europe/Berlin".parse().unwrap();
        let cron = parse_cron("0 3 * * *").unwrap();
        // Berlin moves from CET to CEST on 2026-03-29; 03:00 local stays 03:00 local.
        let times = next_times(&cron, berlin, ms(2026, 3, 27, 12, 0, berlin), 4);
        let local: Vec<(u32, u32)> = times.iter().map(|t| {
            let d = Utc.timestamp_millis_opt(*t as i64).unwrap().with_timezone(&berlin);
            (chrono::Datelike::day(&d), d.hour())
        }).collect();
        assert_eq!(local, vec![(28, 3), (29, 3), (30, 3), (31, 3)]);
        // So the gap in UTC is 23 hours across the change, and 24 hours after it.
        assert_eq!(times[1] - times[0], 23 * 3_600_000);
        assert_eq!(times[2] - times[1], 24 * 3_600_000);
        let weekdays = next_times(&parse_cron("30 9 * * 1-5").unwrap(), Tz::UTC, ms(2026, 11, 27, 10, 0, Tz::UTC), 2);
        // Friday 2026-11-27 after 10:00: next are Monday and Tuesday at 09:30.
        assert_eq!(weekdays, vec![ms(2026, 11, 30, 9, 30, Tz::UTC), ms(2026, 12, 1, 9, 30, Tz::UTC)]);
    }
}
