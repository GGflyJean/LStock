use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone,
    Timelike, Utc, Weekday,
};
use chrono_tz::Tz;

use crate::models::Market;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    PreMarket,
    Trading,
    LunchBreak,
    PostMarket,
    Closed,
    Holiday,
}

impl SessionState {
    pub fn label(self) -> &'static str {
        match self {
            SessionState::PreMarket => "盘前",
            SessionState::Trading => "交易中",
            SessionState::LunchBreak => "午休",
            SessionState::PostMarket => "盘后",
            SessionState::Closed => "已收盘",
            SessionState::Holiday => "休市",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct NextTrigger {
    pub at_utc: DateTime<Utc>,
    pub local_date: NaiveDate,
    pub local_time: NaiveTime,
}

pub fn session_state(market: Market, now_utc: DateTime<Utc>) -> SessionState {
    let local = now_utc.with_timezone(&market.tz());
    if is_market_holiday(market, local.date_naive()) {
        return SessionState::Holiday;
    }
    let minutes = local.hour() * 60 + local.minute();
    match market {
        Market::Cn => {
            if (9 * 60..9 * 60 + 30).contains(&minutes) {
                SessionState::PreMarket
            } else if (9 * 60 + 30..11 * 60 + 30).contains(&minutes) {
                SessionState::Trading
            } else if (11 * 60 + 30..13 * 60).contains(&minutes) {
                SessionState::LunchBreak
            } else if (13 * 60..15 * 60).contains(&minutes) {
                SessionState::Trading
            } else if (15 * 60..15 * 60 + 30).contains(&minutes) {
                SessionState::PostMarket
            } else {
                SessionState::Closed
            }
        }
        Market::Us => {
            if (4 * 60..9 * 60 + 30).contains(&minutes) {
                SessionState::PreMarket
            } else if (9 * 60 + 30..16 * 60).contains(&minutes) {
                SessionState::Trading
            } else if (16 * 60..20 * 60).contains(&minutes) {
                SessionState::PostMarket
            } else {
                SessionState::Closed
            }
        }
    }
}

pub fn is_trading_day(market: Market, date: NaiveDate) -> bool {
    !matches!(date.weekday(), Weekday::Sat | Weekday::Sun) && !is_market_holiday(market, date)
}

pub fn is_market_holiday(market: Market, date: NaiveDate) -> bool {
    let holidays: &[(i32, u32, u32)] = match market {
        Market::Cn => &[
            (2026, 1, 1),
            (2026, 2, 16),
            (2026, 2, 17),
            (2026, 2, 18),
            (2026, 2, 19),
            (2026, 2, 20),
            (2026, 4, 6),
            (2026, 5, 1),
            (2026, 5, 4),
            (2026, 5, 5),
            (2026, 6, 19),
            (2026, 9, 25),
            (2026, 10, 1),
            (2026, 10, 2),
            (2026, 10, 5),
            (2026, 10, 6),
            (2026, 10, 7),
        ],
        Market::Us => &[
            (2026, 1, 1),
            (2026, 1, 19),
            (2026, 2, 16),
            (2026, 4, 3),
            (2026, 5, 25),
            (2026, 6, 19),
            (2026, 7, 3),
            (2026, 9, 7),
            (2026, 11, 26),
            (2026, 12, 25),
        ],
    };
    holidays.iter().any(|(year, month, day)| {
        *year == date.year() && *month == date.month() && *day == date.day()
    })
}

pub fn next_trigger(
    market: Market,
    times: &[NaiveTime],
    now_utc: DateTime<Utc>,
    open_offset_secs: i64,
) -> Option<NextTrigger> {
    let tz = market.tz();
    let now_local = now_utc.with_timezone(&tz);
    let mut sorted = times.to_vec();
    sorted.sort();
    for day_offset in 0..=14 {
        let date = now_local.date_naive() + Duration::days(day_offset);
        if !is_trading_day(market, date) {
            continue;
        }
        for (index, time) in sorted.iter().enumerate() {
            let mut candidate = *time;
            if index == 0 && open_offset_secs >= 0 {
                candidate += Duration::seconds(open_offset_secs);
            }
            let Some(local_dt) = local_datetime(tz, date.and_time(candidate)) else {
                continue;
            };
            let at_utc = local_dt.with_timezone(&Utc);
            if at_utc > now_utc {
                return Some(NextTrigger {
                    at_utc,
                    local_date: date,
                    local_time: candidate,
                });
            }
        }
    }
    None
}

pub fn triggers_for_date(
    market: Market,
    date: NaiveDate,
    times: &[NaiveTime],
    open_offset_secs: i64,
) -> Vec<DateTime<Utc>> {
    if !is_trading_day(market, date) {
        return Vec::new();
    }
    let tz = market.tz();
    let mut sorted = times.to_vec();
    sorted.sort();
    sorted
        .into_iter()
        .enumerate()
        .filter_map(|(index, time)| {
            let mut candidate = time;
            if index == 0 && open_offset_secs >= 0 {
                candidate += Duration::seconds(open_offset_secs);
            }
            local_datetime(tz, date.and_time(candidate)).map(|value| value.with_timezone(&Utc))
        })
        .collect()
}

pub fn local_datetime(tz: Tz, naive: NaiveDateTime) -> Option<DateTime<Tz>> {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(value) => Some(value),
        LocalResult::Ambiguous(first, _) => Some(first),
        LocalResult::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cn_time(hour: u32, minute: u32) -> DateTime<Utc> {
        Market::Cn
            .tz()
            .with_ymd_and_hms(2026, 10, 12, hour, minute, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn us_time(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Market::Us
            .tz()
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn cn_sessions_cover_lunch_and_close() {
        assert_eq!(
            session_state(Market::Cn, cn_time(9, 10)),
            SessionState::PreMarket
        );
        assert_eq!(
            session_state(Market::Cn, cn_time(10, 0)),
            SessionState::Trading
        );
        assert_eq!(
            session_state(Market::Cn, cn_time(12, 0)),
            SessionState::LunchBreak
        );
        assert_eq!(
            session_state(Market::Cn, cn_time(14, 0)),
            SessionState::Trading
        );
        assert_eq!(
            session_state(Market::Cn, cn_time(15, 10)),
            SessionState::PostMarket
        );
        assert_eq!(
            session_state(Market::Cn, cn_time(20, 0)),
            SessionState::Closed
        );
    }

    #[test]
    fn us_sessions_and_closed() {
        assert_eq!(
            session_state(Market::Us, us_time(2026, 10, 12, 8, 0)),
            SessionState::PreMarket
        );
        assert_eq!(
            session_state(Market::Us, us_time(2026, 10, 12, 11, 0)),
            SessionState::Trading
        );
        assert_eq!(
            session_state(Market::Us, us_time(2026, 10, 12, 17, 0)),
            SessionState::PostMarket
        );
        assert_eq!(
            session_state(Market::Us, us_time(2026, 10, 12, 21, 0)),
            SessionState::Closed
        );
    }

    #[test]
    fn weekends_are_holidays() {
        let saturday = NaiveDate::from_ymd_opt(2026, 10, 10).unwrap();
        assert!(is_market_holiday(Market::Cn, saturday) || !is_trading_day(Market::Cn, saturday));
        assert!(!is_trading_day(Market::Us, saturday));
        assert!(!is_trading_day(
            Market::Cn,
            NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()
        ));
        assert!(!is_trading_day(
            Market::Us,
            NaiveDate::from_ymd_opt(2026, 12, 25).unwrap()
        ));
    }

    #[test]
    fn next_cn_trigger_skips_weekend_and_holiday() {
        let times = [
            NaiveTime::from_hms_opt(9, 30, 0).unwrap(),
            NaiveTime::from_hms_opt(13, 0, 0).unwrap(),
            NaiveTime::from_hms_opt(14, 45, 0).unwrap(),
        ];
        let friday_late = cn_time(16, 0);
        let next = next_trigger(Market::Cn, &times, friday_late, 60).unwrap();
        assert_eq!(
            next.local_date,
            NaiveDate::from_ymd_opt(2026, 10, 13).unwrap()
        );
        assert_eq!(next.local_time, NaiveTime::from_hms_opt(9, 31, 0).unwrap());
    }

    #[test]
    fn next_us_trigger_respects_dst() {
        let times = [
            NaiveTime::from_hms_opt(9, 30, 0).unwrap(),
            NaiveTime::from_hms_opt(12, 0, 0).unwrap(),
            NaiveTime::from_hms_opt(15, 45, 0).unwrap(),
        ];
        let friday_after_close = us_time(2026, 3, 6, 17, 0);
        let next = next_trigger(Market::Us, &times, friday_after_close, 60).unwrap();
        assert_eq!(
            next.local_date,
            NaiveDate::from_ymd_opt(2026, 3, 9).unwrap()
        );
        assert_eq!(next.at_utc.format("%H:%M").to_string(), "13:31");
    }

    #[test]
    fn triggers_for_holiday_are_empty() {
        let times = [NaiveTime::from_hms_opt(9, 30, 0).unwrap()];
        let holiday = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        assert!(triggers_for_date(Market::Cn, holiday, &times, 60).is_empty());
    }
}
