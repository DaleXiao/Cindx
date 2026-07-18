use chrono::{Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

const CONFIG_VERSION: u32 = 1;
pub const RUN_HISTORY_LIMIT: usize = 16;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleCadence {
    Once,
    Daily,
    Weekdays,
    Weekly,
}

impl ScheduleCadence {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "once" => Some(Self::Once),
            "daily" => Some(Self::Daily),
            "weekdays" => Some(Self::Weekdays),
            "weekly" => Some(Self::Weekly),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Once => "once",
            Self::Daily => "daily",
            Self::Weekdays => "weekdays",
            Self::Weekly => "weekly",
        }
    }

    pub fn recurring(self) -> bool {
        !matches!(self, Self::Once)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleRunRecord {
    pub id: String,
    pub queue_id: Option<String>,
    pub source: String,
    pub scheduled_for_ms: u64,
    pub queued_at_ms: Option<u64>,
    #[serde(default)]
    pub dispatch_attempts: u32,
    #[serde(default)]
    pub last_dispatch_at_ms: Option<u64>,
    pub started_at_ms: Option<u64>,
    pub finished_at_ms: Option<u64>,
    pub status: String,
    pub error: Option<String>,
}

impl ScheduleRunRecord {
    pub fn active(&self) -> bool {
        matches!(
            self.status.as_str(),
            "preparing" | "queued" | "running" | "waiting_for_permission"
        )
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleRecord {
    pub id: String,
    pub name: String,
    pub project_id: String,
    pub session_id: String,
    pub prompt: String,
    pub effort: String,
    pub timezone: String,
    pub cadence: ScheduleCadence,
    pub anchor_at_ms: u64,
    pub catch_up: bool,
    pub enabled: bool,
    pub next_run_at_ms: Option<u64>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub runs: Vec<ScheduleRunRecord>,
}

impl ScheduleRecord {
    pub fn active_run(&self) -> Option<&ScheduleRunRecord> {
        self.runs.iter().rev().find(|run| run.active())
    }

    pub fn push_run(&mut self, run: ScheduleRunRecord) {
        self.runs.push(run);
        if self.runs.len() > RUN_HISTORY_LIMIT {
            self.runs.drain(..self.runs.len() - RUN_HISTORY_LIMIT);
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleConfig {
    pub version: u32,
    pub schedules: Vec<ScheduleRecord>,
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            schedules: Vec::new(),
        }
    }
}

pub fn parse_timezone(value: &str) -> Result<Tz, String> {
    value
        .trim()
        .parse::<Tz>()
        .map_err(|_| format!("unsupported time zone `{}`", value.trim()))
}

pub fn timestamp_ms_from_local(value: &str, timezone: &str) -> Result<u64, String> {
    let timezone = parse_timezone(timezone)?;
    let local = NaiveDateTime::parse_from_str(value.trim(), "%Y-%m-%dT%H:%M")
        .map_err(|_| "schedule start must use YYYY-MM-DDTHH:MM".to_string())?;
    let resolved = resolve_local_datetime(timezone, local.date(), local.time())
        .ok_or_else(|| "schedule start does not exist in the selected time zone".to_string())?;
    let timestamp_ms = resolved.timestamp_millis();
    if timestamp_ms < 0 {
        return Err("schedule start time is out of range".to_string());
    }
    Ok(timestamp_ms as u64)
}

pub fn initial_next_run_at_ms(
    anchor_at_ms: u64,
    cadence: ScheduleCadence,
    timezone: &str,
    catch_up: bool,
    now_ms: u64,
) -> Result<Option<u64>, String> {
    parse_timezone(timezone)?;
    if anchor_at_ms > now_ms || catch_up {
        return Ok(Some(anchor_at_ms));
    }
    if !cadence.recurring() {
        return Ok(None);
    }
    next_occurrence_after_ms(anchor_at_ms, cadence, timezone, now_ms)
}

pub fn next_occurrence_after_ms(
    anchor_at_ms: u64,
    cadence: ScheduleCadence,
    timezone: &str,
    after_ms: u64,
) -> Result<Option<u64>, String> {
    if !cadence.recurring() {
        return Ok((anchor_at_ms > after_ms).then_some(anchor_at_ms));
    }

    let timezone = parse_timezone(timezone)?;
    let anchor = Utc
        .timestamp_millis_opt(anchor_at_ms as i64)
        .single()
        .ok_or_else(|| "schedule start time is out of range".to_string())?
        .with_timezone(&timezone);
    let after = Utc
        .timestamp_millis_opt(after_ms as i64)
        .single()
        .ok_or_else(|| "schedule comparison time is out of range".to_string())?
        .with_timezone(&timezone);
    let anchor_time = anchor.time();
    let anchor_weekday = anchor.weekday();

    for day_offset in 0..=14 {
        let Some(date) = after
            .date_naive()
            .checked_add_signed(Duration::days(day_offset))
        else {
            break;
        };
        let eligible = match cadence {
            ScheduleCadence::Once => false,
            ScheduleCadence::Daily => true,
            ScheduleCadence::Weekdays => date.weekday().number_from_monday() <= 5,
            ScheduleCadence::Weekly => date.weekday() == anchor_weekday,
        };
        if !eligible {
            continue;
        }
        let Some(candidate) = resolve_local_datetime(timezone, date, anchor_time) else {
            continue;
        };
        let candidate_ms = candidate.timestamp_millis();
        if candidate_ms >= 0 && candidate_ms as u64 > after_ms {
            return Ok(Some(candidate_ms as u64));
        }
    }

    Err("could not calculate the next schedule occurrence".to_string())
}

fn resolve_local_datetime(
    timezone: Tz,
    date: NaiveDate,
    time: NaiveTime,
) -> Option<chrono::DateTime<Tz>> {
    let base = NaiveDateTime::new(date, time);
    for minute_offset in 0..=180 {
        let candidate = base.checked_add_signed(Duration::minutes(minute_offset))?;
        match timezone.from_local_datetime(&candidate) {
            LocalResult::Single(value) => return Some(value),
            LocalResult::Ambiguous(first, second) => return Some(first.min(second)),
            LocalResult::None => continue,
        }
    }
    None
}

pub fn load(path: &Path) -> Result<ScheduleConfig, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ScheduleConfig::default())
        }
        Err(error) => return Err(format!("failed to read schedules: {error}")),
    };
    let mut config = serde_json::from_str::<ScheduleConfig>(&text)
        .map_err(|error| format!("failed to decode schedules: {error}"))?;
    config.version = CONFIG_VERSION;
    for schedule in &mut config.schedules {
        if schedule.runs.len() > RUN_HISTORY_LIMIT {
            schedule
                .runs
                .drain(..schedule.runs.len() - RUN_HISTORY_LIMIT);
        }
    }
    Ok(config)
}

pub fn save(path: &Path, config: &ScheduleConfig) -> Result<(), io::Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    let payload = serde_json::to_vec_pretty(config).map_err(io::Error::other)?;
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&payload)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, TimeZone, Timelike};

    fn local_ms(timezone: Tz, year: i32, month: u32, day: u32, hour: u32) -> u64 {
        timezone
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(year, month, day)
                    .unwrap()
                    .and_hms_opt(hour, 0, 0)
                    .unwrap(),
            )
            .single()
            .unwrap()
            .timestamp_millis() as u64
    }

    #[test]
    fn daily_schedule_keeps_local_time_across_dst() {
        let timezone: Tz = "America/New_York".parse().unwrap();
        let anchor = local_ms(timezone, 2026, 3, 7, 9);
        let after = local_ms(timezone, 2026, 3, 8, 8);
        let next = next_occurrence_after_ms(anchor, ScheduleCadence::Daily, timezone.name(), after)
            .unwrap()
            .unwrap();
        let next = Utc
            .timestamp_millis_opt(next as i64)
            .single()
            .unwrap()
            .with_timezone(&timezone);
        assert_eq!(
            next.date_naive(),
            NaiveDate::from_ymd_opt(2026, 3, 8).unwrap()
        );
        assert_eq!(next.hour(), 9);
    }

    #[test]
    fn weekdays_skip_the_weekend() {
        let timezone: Tz = "Asia/Shanghai".parse().unwrap();
        let anchor = local_ms(timezone, 2026, 7, 17, 9);
        let after = local_ms(timezone, 2026, 7, 17, 10);
        let next =
            next_occurrence_after_ms(anchor, ScheduleCadence::Weekdays, timezone.name(), after)
                .unwrap()
                .unwrap();
        let next = Utc
            .timestamp_millis_opt(next as i64)
            .single()
            .unwrap()
            .with_timezone(&timezone);
        assert_eq!(next.weekday().number_from_monday(), 1);
        assert_eq!(next.hour(), 9);
    }

    #[test]
    fn local_schedule_input_uses_the_selected_timezone() {
        let timestamp = timestamp_ms_from_local("2026-07-18T09:30", "America/New_York").unwrap();
        let timezone: Tz = "America/New_York".parse().unwrap();
        let local = Utc
            .timestamp_millis_opt(timestamp as i64)
            .single()
            .unwrap()
            .with_timezone(&timezone);
        assert_eq!(local.hour(), 9);
        assert_eq!(local.minute(), 30);
    }

    #[test]
    fn config_round_trip_keeps_private_schedule_data() {
        let root = std::env::temp_dir().join(format!(
            "cindx-schedule-test-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("schedules.json");
        let config = ScheduleConfig {
            version: CONFIG_VERSION,
            schedules: vec![ScheduleRecord {
                id: "schedule-one".to_string(),
                name: "Daily review".to_string(),
                project_id: "project-one".to_string(),
                session_id: "session-one".to_string(),
                prompt: "Summarize the workspace".to_string(),
                effort: "auto".to_string(),
                timezone: "Asia/Shanghai".to_string(),
                cadence: ScheduleCadence::Daily,
                anchor_at_ms: 1_800_000_000_000,
                catch_up: true,
                enabled: true,
                next_run_at_ms: Some(1_800_000_000_000),
                created_at_ms: 1,
                updated_at_ms: 1,
                runs: Vec::new(),
            }],
        };
        save(&path, &config).unwrap();
        assert_eq!(load(&path).unwrap(), config);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}
