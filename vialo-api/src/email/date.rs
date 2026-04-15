use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use icu::calendar::Date as IcuDate;
use icu::datetime::fieldsets::{ET, T, YMDT};
use icu::datetime::DateTimeFormatter;
use icu::locale::Locale;
use icu::time::DateTime as IcuDateTime;
use icu::time::Time as IcuTime;

/// Converts DateTime inputs into a locale-aware event timespan string.
pub fn get_event_timespan(
    locale: Locale,
    event_from: Option<DateTime<Utc>>,
    event_to: Option<DateTime<Utc>>,
) -> Option<String> {
    let now = Utc::now();

    let fmt_short_dt = DateTimeFormatter::try_new(
        locale.clone().into(),
        YMDT::short().with_time_precision(icu::datetime::options::TimePrecision::Minute),
    )
    .expect("Failed to create short datetime formatter");

    let fmt_weekday_time = DateTimeFormatter::try_new(
        locale.clone().into(),
        ET::medium()
            .with_alignment(icu::datetime::options::Alignment::Column)
            .with_time_precision(icu::datetime::options::TimePrecision::Minute),
    )
    .expect("Failed to create weekday formatter");

    let fmt_time_only = DateTimeFormatter::try_new(
        locale.into(),
        T::long().with_time_precision(icu::datetime::options::TimePrecision::Minute),
    )
    .expect("Failed to create time formatter");

    // Helper to convert Chrono -> ICU DateTime
    let to_icu = |dt: DateTime<Utc>| IcuDateTime {
        date: IcuDate::try_new_iso(dt.year(), dt.month() as u8, dt.day() as u8).unwrap(),
        time: IcuTime::try_new(
            dt.hour() as u8,
            dt.minute() as u8,
            dt.second() as u8,
            dt.nanosecond(),
        )
        .unwrap(),
    };

    if let Some(from) = event_from {
        let icu_from = to_icu(from);
        let mut ret: String;

        if from.signed_duration_since(now) > Duration::days(7) {
            // Far future
            ret = fmt_short_dt.format(&icu_from).to_string();
        } else {
            // Not so far future
            ret = fmt_weekday_time.format(&icu_from).to_string();
        }

        if let Some(to) = event_to {
            let icu_to = to_icu(to);

            // Same day
            if from.date_naive() == to.date_naive() {
                ret.push_str(" - ");
                ret.push_str(&fmt_time_only.format(&icu_to).to_string());
            } else {
                ret.push_str(" - ");
                ret.push_str(&fmt_short_dt.format(&icu_to).to_string());
            }
        }

        return Some(ret);
    } else if let Some(to) = event_to {
        let icu_to = to_icu(to);
        return Some(format!("until {}", fmt_weekday_time.format(&icu_to)));
    }

    None
}
