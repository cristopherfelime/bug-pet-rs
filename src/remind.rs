use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};

const ALL_DAYS: [u8; 7] = [0, 1, 2, 3, 4, 5, 6];
const SOUND_TYPES: [&str; 4] = ["chime", "bell", "marimba", "soft"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemindKind {
    Alarm,
    Pill,
    Water,
    Todo,
}

impl RemindKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            RemindKind::Alarm => "alarm",
            RemindKind::Pill => "pill",
            RemindKind::Water => "water",
            RemindKind::Todo => "todo",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlarmMode {
    Weekly,
    After,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemindGroup {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemindAlarm {
    pub id: String,
    pub on: bool,
    pub mode: AlarmMode,
    pub days: Vec<u8>,
    pub time: String,
    pub after_min: u32,
    pub repeat: bool,
    pub start_at_ms: i64,
    pub memo: String,
    pub target: String,
    pub chars: Vec<String>,
    pub group: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemindPill {
    pub on: bool,
    pub days: Vec<u8>,
    pub times: Vec<String>,
    pub memo: String,
    pub target: String,
    pub chars: Vec<String>,
    pub track: bool,
    pub taken_date: String,
    pub taken_times: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemindWater {
    pub on: bool,
    pub every_min: u32,
    pub from: String,
    pub to: String,
    pub days: Vec<u8>,
    pub target: String,
    pub chars: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemindTodoItem {
    pub id: String,
    pub text: String,
    pub done: bool,
    pub done_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemindTodo {
    pub items: Vec<RemindTodoItem>,
    pub on: bool,
    pub every_min: u32,
    pub from: String,
    pub to: String,
    pub days: Vec<u8>,
    pub target: String,
    pub chars: Vec<String>,
    pub cheer: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemindSoundEntry {
    pub on: bool,
    pub sound_type: String,
    pub vol: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemindSounds {
    pub alarm: RemindSoundEntry,
    pub pill: RemindSoundEntry,
    pub water: RemindSoundEntry,
    pub todo: RemindSoundEntry,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemindConfig {
    pub groups: Vec<RemindGroup>,
    pub alarms: Vec<RemindAlarm>,
    pub pill: RemindPill,
    pub water: RemindWater,
    pub todo: RemindTodo,
    pub sound: RemindSounds,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemindFireEvent {
    pub target_pet_id: String,
    pub kind: RemindKind,
    pub alarm_id: Option<String>,
    pub slot_hhmm: String,
    pub group_name: String,
    pub memo: String,
    pub left_count: usize,
    pub sound: Option<(String, f64)>,
}

#[derive(Debug, Clone)]
pub struct RemindScheduler {
    pub config: RemindConfig,
    pub dirty_state: bool,
    pub disabled_alarm_ids: Vec<String>,
}

fn parse_hhmm(s: &str) -> Option<(u32, u32)> {
    let b = s.as_bytes();
    if b.len() != 5 || b[2] != b':' {
        return None;
    }
    let hh = s[0..2].parse::<u32>().ok()?;
    let mm = s[3..5].parse::<u32>().ok()?;
    if hh < 24 && mm < 60 {
        Some((hh, mm))
    } else {
        None
    }
}

fn norm_hhmm(val: Option<&Value>, default: &str) -> String {
    if let Some(s) = val.and_then(|v| v.as_str()) {
        if parse_hhmm(s).is_some() {
            return s.to_string();
        }
    }
    default.to_string()
}

fn norm_days(val: Option<&Value>) -> Vec<u8> {
    let Some(arr) = val.and_then(|v| v.as_array()) else {
        return ALL_DAYS.to_vec();
    };
    let mut set = BTreeSet::new();
    for item in arr {
        if let Some(n) = item.as_i64().or_else(|| item.as_f64().map(|f| f as i64)) {
            if (0..=6).contains(&n) {
                set.insert(n as u8);
            }
        }
    }
    set.into_iter().collect()
}

fn norm_memo(val: Option<&Value>) -> String {
    val.and_then(|v| v.as_str())
        .unwrap_or("")
        .chars()
        .take(40)
        .collect()
}

fn norm_target(val: Option<&Value>) -> String {
    if let Some(s) = val.and_then(|v| v.as_str()) {
        if s == "all" {
            return "all".to_string();
        }
        if let Some(rest) = s.strip_prefix("pet") {
            if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
                return s.to_string();
            }
        }
    }
    "all".to_string()
}

fn is_valid_char_id(s: &str) -> bool {
    let len = s.chars().count();
    (1..=64).contains(&len)
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

fn norm_chars(val: Option<&Value>) -> Vec<String> {
    let Some(arr) = val.and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in arr {
        if let Some(s) = item.as_str() {
            if is_valid_char_id(s) && !out.iter().any(|x| x == s) {
                out.push(s.to_string());
                if out.len() >= 20 {
                    break;
                }
            }
        }
    }
    out
}

fn is_valid_group_id(s: &str) -> bool {
    s.strip_prefix('g')
        .is_some_and(|r| (1..=6).contains(&r.len()) && r.bytes().all(|b| b.is_ascii_digit()))
}

fn is_valid_alarm_id(s: &str) -> bool {
    s.strip_prefix('a')
        .is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()))
}

fn is_valid_todo_id(s: &str) -> bool {
    s.strip_prefix('t')
        .is_some_and(|r| (1..=16).contains(&r.len()) && r.bytes().all(|b| b.is_ascii_digit()))
}

fn is_valid_ymd(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && s[0..4].bytes().all(|c| c.is_ascii_digit())
        && s[5..7].bytes().all(|c| c.is_ascii_digit())
        && s[8..10].bytes().all(|c| c.is_ascii_digit())
}

fn norm_sound_entry(val: Option<&Value>) -> RemindSoundEntry {
    let obj = val.and_then(|v| v.as_object());
    let on = obj
        .and_then(|o| o.get("on"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let sound_type = obj
        .and_then(|o| o.get("type"))
        .and_then(|v| v.as_str())
        .filter(|t| SOUND_TYPES.contains(t))
        .unwrap_or("chime")
        .to_string();
    let vol = obj
        .and_then(|o| o.get("vol"))
        .and_then(|v| v.as_f64())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(0.6)
        .clamp(0.05, 1.0);
    RemindSoundEntry {
        on,
        sound_type,
        vol,
    }
}

fn norm_config(value: &Value) -> RemindConfig {
    let now_fallback = Local::now().timestamp_millis();
    let root = value.as_object();

    let mut groups = Vec::new();
    if let Some(arr) = root.and_then(|o| o.get("groups")).and_then(|v| v.as_array()) {
        for g in arr {
            let Some(obj) = g.as_object() else { continue };
            let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if !is_valid_group_id(id) || groups.iter().any(|x: &RemindGroup| x.id == id) {
                continue;
            }
            let cleaned: String = obj
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .chars()
                .filter(|&c| (c as u32) >= 0x20)
                .collect::<String>()
                .trim()
                .chars()
                .take(20)
                .collect();
            if !cleaned.is_empty() {
                groups.push(RemindGroup {
                    id: id.to_string(),
                    name: cleaned,
                });
                if groups.len() >= 12 {
                    break;
                }
            }
        }
    }

    let mut alarms = Vec::new();
    if let Some(arr) = root.and_then(|o| o.get("alarms")).and_then(|v| v.as_array()) {
        for (idx, a) in arr.iter().take(20).enumerate() {
            let obj = a.as_object();
            let raw_id = obj.and_then(|o| o.get("id")).and_then(|v| v.as_str()).unwrap_or("");
            let id = if is_valid_alarm_id(raw_id) {
                raw_id.to_string()
            } else {
                format!("a{}", now_fallback + idx as i64)
            };
            let on = obj
                .and_then(|o| o.get("on"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let mode = match obj.and_then(|o| o.get("mode")).and_then(|v| v.as_str()) {
                Some("after") => AlarmMode::After,
                _ => AlarmMode::Weekly,
            };
            let days = norm_days(obj.and_then(|o| o.get("days")));
            let time = norm_hhmm(obj.and_then(|o| o.get("time")), "09:00");
            let after_min = obj
                .and_then(|o| o.get("afterMin"))
                .and_then(|v| v.as_f64())
                .filter(|v| v.is_finite() && *v > 0.0)
                .map(|v| v.round() as i64)
                .unwrap_or(10)
                .clamp(1, 1440) as u32;
            let repeat = obj
                .and_then(|o| o.get("repeat"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let start_at_ms = obj
                .and_then(|o| o.get("startAt"))
                .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
                .filter(|&v| v != 0)
                .unwrap_or(now_fallback);
            let memo = norm_memo(obj.and_then(|o| o.get("memo")));
            let target = norm_target(obj.and_then(|o| o.get("target")));
            let chars = norm_chars(obj.and_then(|o| o.get("chars")));
            let raw_group = obj
                .and_then(|o| o.get("group"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let group = if is_valid_group_id(raw_group) && groups.iter().any(|g| g.id == raw_group) {
                raw_group.to_string()
            } else {
                String::new()
            };
            alarms.push(RemindAlarm {
                id,
                on,
                mode,
                days,
                time,
                after_min,
                repeat,
                start_at_ms,
                memo,
                target,
                chars,
                group,
            });
        }
    }

    let pill_obj = root.and_then(|o| o.get("pill")).and_then(|v| v.as_object());
    let mut pill_times_set = BTreeSet::new();
    if let Some(arr) = pill_obj.and_then(|o| o.get("times")).and_then(|v| v.as_array()) {
        for v in arr {
            if let Some(s) = v.as_str() {
                if parse_hhmm(s).is_some() {
                    pill_times_set.insert(s.to_string());
                }
            }
        }
    }
    let mut pill_times: Vec<String> = pill_times_set.into_iter().take(8).collect();
    if pill_times.is_empty() {
        pill_times.push("08:00".to_string());
    }
    let taken_obj = pill_obj.and_then(|o| o.get("taken")).and_then(|v| v.as_object());
    let raw_taken_date = taken_obj
        .and_then(|o| o.get("date"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let (taken_date, taken_times) = if is_valid_ymd(raw_taken_date) {
        let times = taken_obj
            .and_then(|o| o.get("times"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .take(8)
                    .collect()
            })
            .unwrap_or_default();
        (raw_taken_date.to_string(), times)
    } else {
        (String::new(), Vec::new())
    };
    let pill = RemindPill {
        on: pill_obj
            .and_then(|o| o.get("on"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        days: norm_days(pill_obj.and_then(|o| o.get("days"))),
        times: pill_times,
        memo: norm_memo(pill_obj.and_then(|o| o.get("memo"))),
        target: norm_target(pill_obj.and_then(|o| o.get("target"))),
        chars: norm_chars(pill_obj.and_then(|o| o.get("chars"))),
        track: pill_obj
            .and_then(|o| o.get("track"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        taken_date,
        taken_times,
    };

    let water_obj = root.and_then(|o| o.get("water")).and_then(|v| v.as_object());
    let water = RemindWater {
        on: water_obj
            .and_then(|o| o.get("on"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        every_min: water_obj
            .and_then(|o| o.get("every"))
            .and_then(|v| v.as_f64())
            .filter(|v| v.is_finite() && *v > 0.0)
            .map(|v| v.round() as i64)
            .unwrap_or(60)
            .clamp(10, 720) as u32,
        from: norm_hhmm(water_obj.and_then(|o| o.get("from")), "09:00"),
        to: norm_hhmm(water_obj.and_then(|o| o.get("to")), "22:00"),
        days: norm_days(water_obj.and_then(|o| o.get("days"))),
        target: norm_target(water_obj.and_then(|o| o.get("target"))),
        chars: norm_chars(water_obj.and_then(|o| o.get("chars"))),
    };

    let todo_obj = root.and_then(|o| o.get("todo")).and_then(|v| v.as_object());
    let mut todo_items = Vec::new();
    if let Some(arr) = todo_obj.and_then(|o| o.get("items")).and_then(|v| v.as_array()) {
        for (idx, item) in arr.iter().enumerate() {
            let Some(obj) = item.as_object() else { continue };
            let raw_id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let id = if is_valid_todo_id(raw_id) {
                raw_id.to_string()
            } else {
                format!("t{}", now_fallback + idx as i64)
            };
            if todo_items.iter().any(|x: &RemindTodoItem| x.id == id) {
                continue;
            }
            let text: String = obj
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .chars()
                .map(|c| if (c as u32) < 0x20 { ' ' } else { c })
                .collect::<String>()
                .trim()
                .chars()
                .take(60)
                .collect();
            if text.is_empty() {
                continue;
            }
            let done = obj.get("done").and_then(|v| v.as_bool()).unwrap_or(false);
            let done_at_ms = if done {
                obj.get("doneAt")
                    .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
                    .unwrap_or(0)
            } else {
                0
            };
            todo_items.push(RemindTodoItem {
                id,
                text,
                done,
                done_at_ms,
            });
            if todo_items.len() >= 50 {
                break;
            }
        }
    }
    let todo = RemindTodo {
        items: todo_items,
        on: todo_obj
            .and_then(|o| o.get("on"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        every_min: todo_obj
            .and_then(|o| o.get("every"))
            .and_then(|v| v.as_f64())
            .filter(|v| v.is_finite() && *v > 0.0)
            .map(|v| v.round() as i64)
            .unwrap_or(60)
            .clamp(10, 720) as u32,
        from: norm_hhmm(todo_obj.and_then(|o| o.get("from")), "09:00"),
        to: norm_hhmm(todo_obj.and_then(|o| o.get("to")), "22:00"),
        days: norm_days(todo_obj.and_then(|o| o.get("days"))),
        target: norm_target(todo_obj.and_then(|o| o.get("target"))),
        chars: norm_chars(todo_obj.and_then(|o| o.get("chars"))),
        cheer: todo_obj
            .and_then(|o| o.get("cheer"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
    };

    let sound_obj = root.and_then(|o| o.get("sound")).and_then(|v| v.as_object());
    let sound = RemindSounds {
        alarm: norm_sound_entry(sound_obj.and_then(|o| o.get("alarm"))),
        pill: norm_sound_entry(sound_obj.and_then(|o| o.get("pill"))),
        water: norm_sound_entry(sound_obj.and_then(|o| o.get("water"))),
        todo: norm_sound_entry(sound_obj.and_then(|o| o.get("todo"))),
    };

    RemindConfig {
        groups,
        alarms,
        pill,
        water,
        todo,
        sound,
    }
}

fn local_at_ms(date: NaiveDate, hhmm: &str) -> Option<i64> {
    let (hh, mm) = parse_hhmm(hhmm)?;
    let time = NaiveTime::from_hms_opt(hh, mm, 0)?;
    let ndt = NaiveDateTime::new(date, time);
    Local
        .from_local_datetime(&ndt)
        .earliest()
        .or_else(|| Local.from_local_datetime(&ndt).latest())
        .map(|dt| dt.timestamp_millis())
}

fn each_local_day<F>(start_ms: i64, end_ms: i64, mut f: F)
where
    F: FnMut(NaiveDate, u8),
{
    let Some(start_dt) = Local.timestamp_millis_opt(start_ms).earliest() else {
        return;
    };
    let mut date = start_dt.date_naive();
    for _ in 0..400 {
        let Some(mid_ms) = local_at_ms(date, "00:00") else {
            break;
        };
        if mid_ms > end_ms {
            break;
        }
        let weekday = date.weekday().num_days_from_sunday() as u8;
        f(date, weekday);
        let Some(next) = date.succ_opt() else {
            break;
        };
        date = next;
    }
}

fn local_hhmm(ms: i64) -> String {
    if let Some(dt) = Local.timestamp_millis_opt(ms).earliest() {
        format!("{:02}:{:02}", dt.hour(), dt.minute())
    } else {
        "00:00".to_string()
    }
}

fn local_ymd(ms: i64) -> Option<String> {
    Local
        .timestamp_millis_opt(ms)
        .earliest()
        .map(|dt| format!("{:04}-{:02}-{:02}", dt.year(), dt.month(), dt.day()))
}

struct DueItem {
    kind: RemindKind,
    at_ms: i64,
    slot_hhmm: String,
    memo: String,
    target: String,
    chars: Vec<String>,
    alarm_id: Option<String>,
    once: bool,
    group_id: String,
    left_count: usize,
}

fn compute_due(config: &RemindConfig, last_ms: i64, now_ms: i64) -> Vec<DueItem> {
    let mut out = Vec::new();
    let mut push_if_due = |kind: RemindKind,
                           at_ms: i64,
                           memo: &str,
                           target: &str,
                           chars: &[String],
                           alarm_id: Option<String>,
                           once: bool,
                           group_id: &str,
                           slot_override: Option<&str>,
                           left_count: usize| {
        if at_ms > last_ms && at_ms <= now_ms {
            out.push(DueItem {
                kind,
                at_ms,
                slot_hhmm: slot_override
                    .map(str::to_string)
                    .unwrap_or_else(|| local_hhmm(at_ms)),
                memo: memo.to_string(),
                target: target.to_string(),
                chars: chars.to_vec(),
                alarm_id,
                once,
                group_id: group_id.to_string(),
                left_count,
            });
        }
    };

    for alarm in &config.alarms {
        if !alarm.on {
            continue;
        }
        match alarm.mode {
            AlarmMode::Weekly => {
                each_local_day(last_ms, now_ms, |date, weekday| {
                    if alarm.days.contains(&weekday) {
                        if let Some(at_ms) = local_at_ms(date, &alarm.time) {
                            push_if_due(
                                RemindKind::Alarm,
                                at_ms,
                                &alarm.memo,
                                &alarm.target,
                                &alarm.chars,
                                Some(alarm.id.clone()),
                                false,
                                &alarm.group,
                                None,
                                0,
                            );
                        }
                    }
                });
            }
            AlarmMode::After => {
                let step_ms = (alarm.after_min as i64) * 60_000;
                if alarm.repeat {
                    let mut n = (((last_ms - alarm.start_at_ms) as f64) / (step_ms as f64))
                        .ceil()
                        .max(1.0) as i64;
                    for _ in 0..2000 {
                        let at_ms = alarm.start_at_ms + n * step_ms;
                        if at_ms > now_ms {
                            break;
                        }
                        push_if_due(
                            RemindKind::Alarm,
                            at_ms,
                            &alarm.memo,
                            &alarm.target,
                            &alarm.chars,
                            Some(alarm.id.clone()),
                            false,
                            &alarm.group,
                            None,
                            0,
                        );
                        n += 1;
                    }
                } else {
                    let at_ms = alarm.start_at_ms + step_ms;
                    push_if_due(
                        RemindKind::Alarm,
                        at_ms,
                        &alarm.memo,
                        &alarm.target,
                        &alarm.chars,
                        Some(alarm.id.clone()),
                        true,
                        &alarm.group,
                        None,
                        0,
                    );
                }
            }
        }
    }

    if config.pill.on {
        each_local_day(last_ms, now_ms, |date, weekday| {
            if config.pill.days.contains(&weekday) {
                for slot in &config.pill.times {
                    if let Some(at_ms) = local_at_ms(date, slot) {
                        push_if_due(
                            RemindKind::Pill,
                            at_ms,
                            &config.pill.memo,
                            &config.pill.target,
                            &config.pill.chars,
                            None,
                            false,
                            "",
                            Some(slot),
                            0,
                        );
                    }
                }
            }
        });
    }

    if config.water.on {
        let step_ms = (config.water.every_min as i64) * 60_000;
        each_local_day(last_ms - 86_400_000, now_ms, |date, weekday| {
            if !config.water.days.contains(&weekday) {
                return;
            }
            let (Some(start_ms), Some(mut end_ms)) = (
                local_at_ms(date, &config.water.from),
                local_at_ms(date, &config.water.to),
            ) else {
                return;
            };
            if end_ms <= start_ms {
                end_ms += 86_400_000;
            }
            let mut t = start_ms;
            while t <= end_ms {
                push_if_due(
                    RemindKind::Water,
                    t,
                    "",
                    &config.water.target,
                    &config.water.chars,
                    None,
                    false,
                    "",
                    None,
                    0,
                );
                t += step_ms;
            }
        });
    }

    let undone: Vec<&RemindTodoItem> = config.todo.items.iter().filter(|it| !it.done).collect();
    if config.todo.on && !undone.is_empty() {
        let step_ms = (config.todo.every_min as i64) * 60_000;
        let first_text = &undone[0].text;
        let left_count = undone.len();
        each_local_day(last_ms - 86_400_000, now_ms, |date, weekday| {
            if !config.todo.days.contains(&weekday) {
                return;
            }
            let (Some(start_ms), Some(mut end_ms)) = (
                local_at_ms(date, &config.todo.from),
                local_at_ms(date, &config.todo.to),
            ) else {
                return;
            };
            if end_ms <= start_ms {
                end_ms += 86_400_000;
            }
            let mut t = start_ms + step_ms;
            while t <= end_ms {
                push_if_due(
                    RemindKind::Todo,
                    t,
                    first_text,
                    &config.todo.target,
                    &config.todo.chars,
                    None,
                    false,
                    "",
                    None,
                    left_count,
                );
                t += step_ms;
            }
        });
    }

    out.sort_by_key(|d| d.at_ms);
    out
}

fn resolve_targets(
    target: &str,
    chars: &[String],
    pets: &[(String, String, bool)],
) -> Vec<String> {
    let visible: Vec<&(String, String, bool)> = pets.iter().filter(|p| !p.2).collect();
    if !chars.is_empty() {
        let matched: Vec<String> = visible
            .iter()
            .filter(|p| chars.iter().any(|c| c == &p.1))
            .map(|p| p.0.clone())
            .collect();
        if !matched.is_empty() {
            return matched;
        }
        return visible.iter().map(|p| p.0.clone()).collect();
    }
    if target != "all" {
        if visible.iter().any(|p| p.0 == target) {
            return vec![target.to_string()];
        }
    }
    visible.iter().map(|p| p.0.clone()).collect()
}

impl RemindScheduler {
    pub fn from_json(value: &Value) -> Self {
        Self {
            config: norm_config(value),
            dirty_state: false,
            disabled_alarm_ids: Vec::new(),
        }
    }

    #[allow(dead_code)]
    pub fn reload(&mut self, value: &Value) {
        self.config = norm_config(value);
    }

    pub fn reset_pill_taken_if_new_day(&mut self, today_date_str: &str) -> bool {
        if self.config.pill.track
            && !self.config.pill.taken_date.is_empty()
            && self.config.pill.taken_date != today_date_str
        {
            self.config.pill.taken_date = today_date_str.to_string();
            self.config.pill.taken_times.clear();
            self.dirty_state = true;
            return true;
        }
        false
    }

    pub fn mark_pill_taken(&mut self, date_str: &str, slot_hhmm: &str) -> bool {
        if !self.config.pill.track {
            return false;
        }
        let mut changed = false;
        if self.config.pill.taken_date != date_str {
            self.config.pill.taken_date = date_str.to_string();
            self.config.pill.taken_times.clear();
            changed = true;
        }
        if !self.config.pill.taken_times.iter().any(|s| s == slot_hhmm)
            && self.config.pill.taken_times.len() < 8
        {
            self.config.pill.taken_times.push(slot_hhmm.to_string());
            changed = true;
        }
        if changed {
            self.dirty_state = true;
        }
        changed
    }

    #[allow(dead_code)]
    pub fn take_dirty(&mut self) -> bool {
        let was = self.dirty_state;
        self.dirty_state = false;
        was
    }

    pub fn take_disabled_alarm_ids(&mut self) -> Vec<String> {
        std::mem::take(&mut self.disabled_alarm_ids)
    }

    pub fn tick(
        &mut self,
        last_ms: i64,
        now_ms: i64,
        pets: &[(String, String, bool)],
    ) -> Vec<RemindFireEvent> {
        if let Some(today) = local_ymd(now_ms) {
            self.reset_pill_taken_if_new_day(&today);
        }
        if now_ms <= last_ms {
            return Vec::new();
        }

        let mut due_items = compute_due(&self.config, last_ms, now_ms);
        if now_ms - last_ms > 180_000 && !due_items.is_empty() {
            let mut dedup: HashMap<(RemindKind, String), DueItem> = HashMap::new();
            for item in due_items {
                let key = (item.kind, item.alarm_id.clone().unwrap_or_default());
                dedup.insert(key, item);
            }
            due_items = dedup.into_values().collect();
            due_items.sort_by_key(|d| d.at_ms);
        }

        let mut events = Vec::new();
        for item in due_items {
            let sound_entry = match item.kind {
                RemindKind::Alarm => &self.config.sound.alarm,
                RemindKind::Pill => &self.config.sound.pill,
                RemindKind::Water => &self.config.sound.water,
                RemindKind::Todo => &self.config.sound.todo,
            };
            let mut sound = if sound_entry.on {
                Some((sound_entry.sound_type.clone(), sound_entry.vol))
            } else {
                None
            };
            let group_name = if item.group_id.is_empty() {
                String::new()
            } else {
                self.config
                    .groups
                    .iter()
                    .find(|g| g.id == item.group_id)
                    .map(|g| g.name.clone())
                    .unwrap_or_default()
            };

            let targets = resolve_targets(&item.target, &item.chars, pets);
            for target_pet_id in targets {
                events.push(RemindFireEvent {
                    target_pet_id,
                    kind: item.kind,
                    alarm_id: item.alarm_id.clone(),
                    slot_hhmm: item.slot_hhmm.clone(),
                    group_name: group_name.clone(),
                    memo: item.memo.clone(),
                    left_count: item.left_count,
                    sound: sound.take(),
                });
            }

            if item.once {
                if let Some(ref aid) = item.alarm_id {
                    if let Some(alarm) = self.config.alarms.iter_mut().find(|a| &a.id == aid) {
                        if alarm.on {
                            alarm.on = false;
                            self.dirty_state = true;
                            if !self.disabled_alarm_ids.contains(aid) {
                                self.disabled_alarm_ids.push(aid.clone());
                            }
                        }
                    }
                }
            }
        }
        events
    }

    #[allow(dead_code)]
    pub fn next_fire_ms(&self, kind: RemindKind, from_ms: i64) -> Option<i64> {
        let mut filtered = self.config.clone();
        if kind != RemindKind::Alarm {
            filtered.alarms.clear();
        }
        if kind != RemindKind::Pill {
            filtered.pill.on = false;
        }
        if kind != RemindKind::Water {
            filtered.water.on = false;
        }
        if kind != RemindKind::Todo {
            filtered.todo.on = false;
        }
        compute_due(&filtered, from_ms, from_ms + 691_200_000)
            .first()
            .map(|d| d.at_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_local_ms(y: i32, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> i64 {
        let date = NaiveDate::from_ymd_opt(y, m, d).unwrap();
        let time = NaiveTime::from_hms_opt(hh, mm, ss).unwrap();
        Local
            .from_local_datetime(&NaiveDateTime::new(date, time))
            .earliest()
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn weekly_alarm_and_sound_target_resolution() {
        // 2026-11-27 is Friday -> weekday().num_days_from_sunday() == 5
        let friday = NaiveDate::from_ymd_opt(2026, 11, 27).unwrap();
        let wd = friday.weekday().num_days_from_sunday() as u8;

        let val = json!({
            "groups": [{ "id": "g1", "name": "Work" }],
            "alarms": [{
                "id": "a1",
                "on": true,
                "mode": "weekly",
                "days": [wd],
                "time": "09:30",
                "memo": "Standup",
                "target": "all",
                "chars": ["AlphaPet"],
                "group": "g1"
            }],
            "sound": {
                "alarm": { "on": true, "type": "bell", "vol": 0.8 }
            }
        });

        let mut sched = RemindScheduler::from_json(&val);
        let pets = vec![
            ("pet1".to_string(), "AlphaPet".to_string(), false),
            ("pet2".to_string(), "AlphaPet".to_string(), false),
            ("pet3".to_string(), "BetaPet".to_string(), false),
        ];
        let t0 = make_local_ms(2026, 11, 27, 9, 29, 50);
        let t1 = make_local_ms(2026, 11, 27, 9, 30, 5);
        let events = sched.tick(t0, t1, &pets);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].target_pet_id, "pet1");
        assert_eq!(events[0].kind, RemindKind::Alarm);
        assert_eq!(events[0].group_name, "Work");
        assert_eq!(events[0].memo, "Standup");
        assert_eq!(events[0].sound, Some(("bell".to_string(), 0.8)));
        assert_eq!(events[1].target_pet_id, "pet2");
        assert_eq!(events[1].sound, None);

        // Chars fallback when no visible pet matches chars
        let pets_other = vec![("pet3".to_string(), "GammaPet".to_string(), false)];
        let events2 = sched.tick(t0, t1, &pets_other);
        assert_eq!(events2.len(), 1);
        assert_eq!(events2[0].target_pet_id, "pet3");
    }

    #[test]
    fn oneshot_after_alarm_auto_disables() {
        let start = make_local_ms(2026, 11, 27, 10, 0, 0);
        let val = json!({
            "alarms": [{
                "id": "a99",
                "on": true,
                "mode": "after",
                "afterMin": 5,
                "repeat": false,
                "startAt": start,
                "memo": "Tea ready"
            }]
        });
        let mut sched = RemindScheduler::from_json(&val);
        let pets = vec![("pet1".to_string(), "AlphaPet".to_string(), false)];

        // Exact 5 min mark: 10:04:55 -> 10:05:05
        let ev = sched.tick(start + 295_000, start + 305_000, &pets);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].alarm_id.as_deref(), Some("a99"));
        assert!(!sched.config.alarms[0].on);
        assert!(sched.take_dirty());
        assert_eq!(sched.take_disabled_alarm_ids(), vec!["a99".to_string()]);
        assert!(sched.tick(start + 295_000, start + 305_000, &pets).is_empty());
    }

    #[test]
    fn midnight_crossing_water_schedule_fires_after_midnight() {
        // 2026-11-27 is Friday (day 5); 2026-11-28 is Saturday (day 6, NOT in days)
        let fri = NaiveDate::from_ymd_opt(2026, 11, 27).unwrap();
        let fri_wd = fri.weekday().num_days_from_sunday() as u8;
        let val = json!({
            "water": {
                "on": true,
                "every": 60,
                "from": "22:00",
                "to": "03:00",
                "days": [fri_wd],
                "target": "pet2"
            }
        });
        let mut sched = RemindScheduler::from_json(&val);
        let pets = vec![
            ("pet1".to_string(), "AlphaPet".to_string(), false),
            ("pet2".to_string(), "BetaPet".to_string(), false),
        ];
        // Check 00:59:50 -> 01:00:10 on Saturday morning
        let t0 = make_local_ms(2026, 11, 28, 0, 59, 50);
        let t1 = make_local_ms(2026, 11, 28, 1, 0, 10);
        let ev = sched.tick(t0, t1, &pets);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].target_pet_id, "pet2");
        assert_eq!(ev[0].kind, RemindKind::Water);
        assert_eq!(ev[0].slot_hhmm, "01:00");
    }

    #[test]
    fn pill_taken_resets_across_day_boundary() {
        let val = json!({
            "pill": {
                "on": true,
                "times": ["08:00", "20:00"],
                "track": true,
                "taken": { "date": "2026-11-26", "times": ["08:00"] }
            }
        });
        let mut sched = RemindScheduler::from_json(&val);
        let pets = vec![("pet1".to_string(), "AlphaPet".to_string(), false)];
        let t0 = make_local_ms(2026, 11, 27, 7, 59, 50);
        let t1 = make_local_ms(2026, 11, 27, 8, 0, 10);
        let ev = sched.tick(t0, t1, &pets);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].slot_hhmm, "08:00");
        assert_eq!(sched.config.pill.taken_date, "2026-11-27");
        assert!(sched.config.pill.taken_times.is_empty());
        assert!(sched.take_dirty());

        assert!(sched.mark_pill_taken("2026-11-27", "08:00"));
        assert!(!sched.mark_pill_taken("2026-11-27", "08:00"));
        assert_eq!(sched.config.pill.taken_times, vec!["08:00".to_string()]);
    }

    #[test]
    fn over_180s_gap_dedups_keeping_latest() {
        let fri = NaiveDate::from_ymd_opt(2026, 11, 27).unwrap();
        let fri_wd = fri.weekday().num_days_from_sunday() as u8;
        let val = json!({
            "water": {
                "on": true,
                "every": 30,
                "from": "09:00",
                "to": "12:00",
                "days": [fri_wd]
            }
        });
        let mut sched = RemindScheduler::from_json(&val);
        let pets = vec![("pet1".to_string(), "AlphaPet".to_string(), false)];
        // 2-hour sleep jump covering 09:00, 09:30, 10:00, 10:30
        let t0 = make_local_ms(2026, 11, 27, 8, 55, 0);
        let t1 = make_local_ms(2026, 11, 27, 10, 35, 0);
        let ev = sched.tick(t0, t1, &pets);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, RemindKind::Water);
        assert_eq!(ev[0].slot_hhmm, "10:30");
    }
}
