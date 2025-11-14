//! Time specification parser for user-friendly time expressions
//!
//! Supports formats like journalctl:
//! - "now" - current time
//! - "today" - start of today
//! - "yesterday" - start of yesterday
//! - "-1h", "-2hours" - relative time (hours)
//! - "-1d", "-2days" - relative time (days)
//! - "-1w", "-2weeks" - relative time (weeks)
//! - "2025-01-12" - specific date
//! - "2025-01-12 14:30:00" - specific datetime

use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone};

/// Parse a time specification string and return a unix timestamp (seconds since epoch)
pub fn parse_time_spec(spec: &str) -> Result<u32> {
    let spec = spec.trim().to_lowercase();

    // Handle special keywords
    let dt = match spec.as_str() {
        "now" => Local::now(),
        "today" => Local::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| anyhow!("Failed to create datetime for today"))?
            .and_local_timezone(Local)
            .single()
            .ok_or_else(|| anyhow!("Ambiguous timezone for today"))?,
        "yesterday" => {
            let yesterday = Local::now().date_naive() - Duration::days(1);
            yesterday
                .and_hms_opt(0, 0, 0)
                .ok_or_else(|| anyhow!("Failed to create datetime for yesterday"))?
                .and_local_timezone(Local)
                .single()
                .ok_or_else(|| anyhow!("Ambiguous timezone for yesterday"))?
        }
        _ => {
            // Try parsing as relative time (e.g., "-1h", "-2days")
            if let Some(relative) = parse_relative_time(&spec)? {
                return Ok(relative);
            }

            // Try parsing as absolute date/datetime
            parse_absolute_time(&spec)?
        }
    };

    // Convert to unix timestamp
    let timestamp = dt.timestamp();
    if timestamp < 0 {
        return Err(anyhow!("Time is before unix epoch"));
    }
    if timestamp > u32::MAX as i64 {
        return Err(anyhow!("Time is too far in the future"));
    }

    Ok(timestamp as u32)
}

/// Parse relative time expressions like "-1h", "-2days", "-3weeks"
fn parse_relative_time(spec: &str) -> Result<Option<u32>> {
    if !spec.starts_with('-') {
        return Ok(None);
    }

    let spec = &spec[1..]; // Remove leading '-'

    // Try to parse different formats
    let (value, unit) = if let Some(pos) = spec.find(|c: char| !c.is_ascii_digit()) {
        let (num_str, unit_str) = spec.split_at(pos);
        let value: i64 = num_str
            .parse()
            .map_err(|_| anyhow!("Invalid number in relative time: {}", num_str))?;
        (value, unit_str)
    } else {
        return Err(anyhow!("No time unit specified in relative time"));
    };

    // Parse the unit
    let duration = match unit {
        "s" | "sec" | "second" | "seconds" => Duration::seconds(value),
        "m" | "min" | "minute" | "minutes" => Duration::minutes(value),
        "h" | "hour" | "hours" => Duration::hours(value),
        "d" | "day" | "days" => Duration::days(value),
        "w" | "week" | "weeks" => Duration::weeks(value),
        _ => return Err(anyhow!("Unknown time unit: {}", unit)),
    };

    let now = Local::now();
    let target = now - duration;

    let timestamp = target.timestamp();
    if timestamp < 0 {
        return Err(anyhow!("Relative time results in date before unix epoch"));
    }
    if timestamp > u32::MAX as i64 {
        return Err(anyhow!("Time is too far in the future"));
    }

    Ok(Some(timestamp as u32))
}

/// Parse absolute time expressions like "2025-01-12" or "2025-01-12 14:30:00"
fn parse_absolute_time(spec: &str) -> Result<DateTime<Local>> {
    // Try date formats
    let formats = [
        // Date only
        "%Y-%m-%d",
        "%Y/%m/%d",
        "%d.%m.%Y",
        // Date and time
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y/%m/%d %H:%M:%S",
        "%Y/%m/%d %H:%M",
    ];

    for format in &formats {
        // Try parsing as full datetime first
        if let Ok(dt) = NaiveDateTime::parse_from_str(spec, format) {
            // Convert to local timezone
            return Local
                .from_local_datetime(&dt)
                .single()
                .ok_or_else(|| anyhow!("Ambiguous timezone for datetime"));
        }

        // Try parsing as date only
        if let Ok(date) = NaiveDate::parse_from_str(spec, format) {
            // Start of day in local timezone
            let dt = date
                .and_hms_opt(0, 0, 0)
                .ok_or_else(|| anyhow!("Failed to create datetime"))?;
            return Local
                .from_local_datetime(&dt)
                .single()
                .ok_or_else(|| anyhow!("Ambiguous timezone for date"));
        }
    }

    Err(anyhow!("Could not parse time specification: {}", spec))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_now() {
        let result = parse_time_spec("now").unwrap();
        let now = Local::now().timestamp() as u32;
        assert!((result as i64 - now as i64).abs() < 2); // Within 2 seconds
    }

    #[test]
    fn test_parse_today() {
        let result = parse_time_spec("today").unwrap();
        let today = Local::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_local_timezone(Local)
            .single()
            .unwrap()
            .timestamp() as u32;
        assert_eq!(result, today);
    }

    #[test]
    fn test_parse_relative_hours() {
        let result = parse_time_spec("-2h").unwrap();
        let expected = (Local::now() - Duration::hours(2)).timestamp() as u32;
        assert!((result as i64 - expected as i64).abs() < 2);
    }

    #[test]
    fn test_parse_relative_days() {
        let result = parse_time_spec("-7days").unwrap();
        let expected = (Local::now() - Duration::days(7)).timestamp() as u32;
        assert!((result as i64 - expected as i64).abs() < 2);
    }

    #[test]
    fn test_parse_absolute_date() {
        let result = parse_time_spec("2025-01-12").unwrap();
        let expected = Local
            .with_ymd_and_hms(2025, 1, 12, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp() as u32;
        assert_eq!(result, expected);
    }

    #[test]
    fn test_parse_absolute_datetime() {
        let result = parse_time_spec("2025-01-12 14:30:00").unwrap();
        let expected = Local
            .with_ymd_and_hms(2025, 1, 12, 14, 30, 0)
            .single()
            .unwrap()
            .timestamp() as u32;
        assert_eq!(result, expected);
    }
}
