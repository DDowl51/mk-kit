//! 中文（和几个英文）的日期写法 → 日期。纯逻辑，不知道「今天」：调用方给。
//!
//! 和业务无关：stash 的 `@明天`、dida 的 `@周五` 和 `dida due <id> 11-20` 都用它。

use jiff::civil::{Date, Weekday};
use jiff::tz::{Offset, TimeZone};
use jiff::{Span, Timestamp};
use kernel_api::AppError;

/// 一个时刻（UTC 毫秒）在偏移 `offset_min` 分钟的本地是哪天。
pub fn local_date(utc_ms: i64, offset_min: i32) -> Result<Date, AppError> {
    let ts = Timestamp::from_millisecond(utc_ms)
        .map_err(|e| AppError::validation(format!("时刻 {utc_ms} 不对：{e}")))?;
    let offset = Offset::from_seconds(offset_min.saturating_mul(60))
        .map_err(|e| AppError::validation(format!("时区偏移 {offset_min} 分钟不对：{e}")))?;
    Ok(ts.to_zoned(TimeZone::fixed(offset)).date())
}

/// 星期几的中文：`周一` … `周日`。
pub const fn weekday_name(w: Weekday) -> &'static str {
    match w {
        Weekday::Monday => "周一",
        Weekday::Tuesday => "周二",
        Weekday::Wednesday => "周三",
        Weekday::Thursday => "周四",
        Weekday::Friday => "周五",
        Weekday::Saturday => "周六",
        Weekday::Sunday => "周日",
    }
}

fn plus(today: Date, days: i64) -> Option<Date> {
    today.checked_add(Span::new().try_days(days).ok()?).ok()
}

fn weekday(word: &str) -> Option<Weekday> {
    Some(match word {
        "一" | "mon" | "monday" => Weekday::Monday,
        "二" | "tue" | "tuesday" => Weekday::Tuesday,
        "三" | "wed" | "wednesday" => Weekday::Wednesday,
        "四" | "thu" | "thursday" => Weekday::Thursday,
        "五" | "fri" | "friday" => Weekday::Friday,
        "六" | "sat" | "saturday" => Weekday::Saturday,
        "日" | "天" | "sun" | "sunday" => Weekday::Sunday,
        _ => return None,
    })
}

/// 周 X：今天之后最近的那一天（不含今天：今天就写 `@今天`）。
fn next_weekday(today: Date, day: Weekday) -> Option<Date> {
    let ahead = (i64::from(day.to_monday_zero_offset())
        - i64::from(today.weekday().to_monday_zero_offset()))
    .rem_euclid(7);
    plus(today, if ahead == 0 { 7 } else { ahead })
}

/// 下周 X：下一个自然周（周一开始）里的那一天。
fn weekday_next_week(today: Date, day: Weekday) -> Option<Date> {
    let to_monday = 7 - i64::from(today.weekday().to_monday_zero_offset());
    plus(today, to_monday + i64::from(day.to_monday_zero_offset()))
}

/// 月和日：今年的；已经过了就是明年的。
fn month_day(today: Date, month: &str, day: &str) -> Option<Date> {
    let (month, day) = (month.parse::<i8>().ok()?, day.parse::<i8>().ok()?);
    let this_year = Date::new(today.year(), month, day).ok()?;
    if this_year >= today {
        Some(this_year)
    } else {
        Date::new(today.year().checked_add(1)?, month, day).ok()
    }
}

/// 一个词 → 日期。认不出返回 `None`（调用方决定怎么办，比如原样留在文字里）。
///
/// 认的写法：今天 / 明天 / 后天 / 大后天、today / tomorrow；
/// 周五 / 星期五 / 礼拜五 / fri / friday、下周五；
/// 3天后 / +3；2026-11-20、11-20、11/20、11月20日 / 11月20号。
pub fn parse(word: &str, today: Date) -> Option<Date> {
    let word = word.trim().to_lowercase();
    let w = word.as_str();
    match w {
        "今天" | "today" => return Some(today),
        "明天" | "tomorrow" => return plus(today, 1),
        "后天" => return plus(today, 2),
        "大后天" => return plus(today, 3),
        _ => {}
    }
    if let Some(day) = weekday(w) {
        // 英文缩写单独出现时也认（周几的中文要带前缀，单个「一」不算）
        if w.is_ascii() {
            return next_weekday(today, day);
        }
    }
    for prefix in ["下周", "下星期", "下礼拜"] {
        if let Some(rest) = w.strip_prefix(prefix) {
            return weekday_next_week(today, weekday(rest)?);
        }
    }
    for prefix in ["周", "星期", "礼拜"] {
        if let Some(rest) = w.strip_prefix(prefix) {
            return next_weekday(today, weekday(rest)?);
        }
    }
    if let Some(n) = w.strip_suffix("天后").or_else(|| w.strip_prefix('+')) {
        let n: i64 = n.parse().ok()?;
        return (1..=3650).contains(&n).then(|| plus(today, n)).flatten();
    }
    if let Some(rest) = w.strip_suffix('日').or_else(|| w.strip_suffix('号')) {
        let (month, day) = rest.split_once('月')?;
        return month_day(today, month, day);
    }
    let parts: Vec<&str> = w.split(['-', '/']).collect();
    match parts.as_slice() {
        [y, m, d] if y.len() == 4 => {
            Date::new(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?).ok()
        }
        [m, d] if m.len() <= 2 && d.len() <= 2 => month_day(today, m, d),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        s.parse().unwrap()
    }

    // 2026-11-18 是周三
    const TODAY: &str = "2026-11-18";

    #[test]
    fn words() {
        let today = d(TODAY);
        for (word, want) in [
            ("今天", "2026-11-18"),
            ("today", "2026-11-18"),
            ("明天", "2026-11-19"),
            ("Tomorrow", "2026-11-19"),
            ("后天", "2026-11-20"),
            ("大后天", "2026-11-21"),
            ("周五", "2026-11-20"),
            ("星期五", "2026-11-20"),
            ("礼拜五", "2026-11-20"),
            ("fri", "2026-11-20"),
            ("周一", "2026-11-23"),
            ("周日", "2026-11-22"),
            ("周天", "2026-11-22"),
            ("周三", "2026-11-25"), // 今天是周三：指下一个周三
            ("下周一", "2026-11-23"),
            ("下周三", "2026-11-25"),
            ("下周日", "2026-11-29"),
            ("3天后", "2026-11-21"),
            ("+14", "2026-12-02"),
            ("2026-11-20", "2026-11-20"),
            ("2027-02-01", "2027-02-01"),
            ("11-20", "2026-11-20"),
            ("11/20", "2026-11-20"),
            ("1-5", "2027-01-05"), // 今年的已经过了
            ("11-18", "2026-11-18"),
            ("12月1日", "2026-12-01"),
            ("3月2号", "2027-03-02"),
        ] {
            assert_eq!(parse(word, today), Some(d(want)), "{word}");
        }
    }

    #[test]
    fn not_dates() {
        let today = d(TODAY);
        for word in [
            "老王",
            "一",
            "周八",
            "下周",
            "2-30",
            "2026-02-30",
            "13-01",
            "+0",
            "+abc",
            "x天后",
            "",
            "11-20-3",
        ] {
            assert_eq!(parse(word, today), None, "{word}");
        }
    }

    #[test]
    fn next_week_from_sunday() {
        // 周日的「下周一」是明天
        assert_eq!(parse("下周一", d("2026-11-22")), Some(d("2026-11-23")));
    }

    #[test]
    fn local_date_uses_the_offset() {
        let utc_ms = 1_794_933_000_000; // 2026-11-17T16:30Z
        assert_eq!(local_date(utc_ms, 540).unwrap(), d("2026-11-18"));
        assert_eq!(local_date(utc_ms, 0).unwrap(), d("2026-11-17"));
    }

    #[test]
    fn weekday_names() {
        assert_eq!(weekday_name(d(TODAY).weekday()), "周三");
    }
}
